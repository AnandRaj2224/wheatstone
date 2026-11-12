// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;
use typst::{World, WorldExt, diag::SourceDiagnostic};
use typst_ide::Tooltip as IdeTooltip;
use typst_library::diag::Severity;

use super::world::TypesetterWorld;
use crate::utils;

#[derive(Clone, Debug)]
pub enum CompilerError {
    SendError(String),
    RecvError(String),
    CompilationFailed(String),
    InvalidPdfStandard(String),
    Cancelled,
}

impl std::fmt::Display for CompilerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendError(e) => write!(f, "Send error: {}", e),
            Self::RecvError(e) => write!(f, "Receive error: {}", e),
            Self::CompilationFailed(e) => write!(f, "{}", e),
            Self::InvalidPdfStandard(s) => write!(f, "Invalid PDF standard: {}", s),
            Self::Cancelled => write!(f, "Cancelled"),
        }
    }
}

impl std::error::Error for CompilerError {}

#[derive(Clone, Debug)]
pub struct PageMetadata {
    pub page_number: usize,
    pub dimensions: (f32, f32), // (width_pt, height_pt)
}

#[derive(Clone, Debug)]
pub struct PreviewResult {
    pub generation: u64,
    pub page_metadata: Vec<PageMetadata>,
    pub diagnostics: Vec<PreviewDiagnostic>,
    pub dependencies: Vec<PathBuf>,
}

pub struct PageImage {
    pub page_number: usize,
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u8>,
}

impl Drop for PageImage {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        {
            let size_kb = self.pixels.len() / 1024;
            println!(
                "PageImage: Dropping page {} ({} KB)",
                self.page_number, size_kb
            );
        }

        // Explicitly clear the pixel buffer
        self.pixels.clear();
        self.pixels.shrink_to_fit();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    FileNotFound,
    FileAccessDenied,
    FileIsDirectory,
    FileNotSource,
    FileInvalidUtf8,
    PackageError,
    CompilationError,
    Other,
}

#[derive(Clone, Debug)]
pub struct PreviewDiagnostic {
    pub filename: Option<String>,
    pub hints: Vec<String>,
    pub location_range: Option<(usize, usize)>,
    pub message: String,
    pub severity: Severity,
    pub kind: DiagnosticKind,
}

impl PreviewDiagnostic {
    pub fn new(diag: &SourceDiagnostic, world: &TypesetterWorld) -> Self {
        let range = Self::compute_range(diag, world);
        let filename = Self::compute_filename(diag, world);
        let raw_message = diag.message.to_string();
        let severity = diag.severity;
        let kind = Self::classify_diagnostic(&raw_message, severity);

        // Sanitize file-not-found messages to remove confusing sandbox/virtual paths.
        let message = if kind == DiagnosticKind::FileNotFound {
            Self::sanitize_file_not_found_message(&raw_message)
        } else {
            raw_message
        };

        Self {
            filename,
            hints: diag.hints.iter().map(|h| h.v.to_string()).collect(),
            location_range: range,
            message,
            severity,
            kind,
        }
    }

    fn classify_diagnostic(message: &str, severity: Severity) -> DiagnosticKind {
        if !matches!(severity, Severity::Error) {
            return DiagnosticKind::Other;
        }

        let msg_lower = message.to_lowercase();

        // Check for package errors first, before file errors, since package
        // error messages may also contain file-related substrings like "not found".
        if msg_lower.contains("failed to load package")
            || (msg_lower.contains("package")
                && (msg_lower.contains("not found")
                    || msg_lower.contains("could not load")
                    || msg_lower.contains("does not exist")))
        {
            return DiagnosticKind::PackageError;
        }

        if msg_lower.contains("file not found")
            || (msg_lower.contains("not found") && msg_lower.contains("file"))
        {
            DiagnosticKind::FileNotFound
        } else if msg_lower.contains("access denied")
            || msg_lower.contains("could not access")
            || msg_lower.contains("permission denied")
        {
            DiagnosticKind::FileAccessDenied
        } else if msg_lower.contains("is a directory") {
            DiagnosticKind::FileIsDirectory
        } else if msg_lower.contains("not a typst source file") || msg_lower.contains("not source")
        {
            DiagnosticKind::FileNotSource
        } else if msg_lower.contains("invalid utf-8") || msg_lower.contains("not valid utf-8") {
            DiagnosticKind::FileInvalidUtf8
        } else if msg_lower.contains("expected")
            || msg_lower.contains("unexpected")
            || msg_lower.contains("undefined")
            || msg_lower.contains("unknown")
            || (msg_lower.contains("missing") && !msg_lower.contains("file"))
        {
            DiagnosticKind::CompilationError
        } else {
            DiagnosticKind::Other
        }
    }

    /// Rewrites Typst's "file not found (searched at /some/sandbox/path/file.typ)"
    /// into the friendlier "file not found: file.typ", stripping sandbox/virtual paths
    /// that would be meaningless or confusing to the user.
    fn sanitize_file_not_found_message(message: &str) -> String {
        if let Some(paren_start) = message.find('(') {
            let prefix = message[..paren_start].trim();
            let inside = &message[paren_start + 1..];
            if let Some(paren_end) = inside.rfind(')') {
                let inner = inside[..paren_end].trim();
                let path_str = inner.strip_prefix("searched at ").unwrap_or(inner).trim();
                let path = std::path::Path::new(path_str);
                // Only show the filename if the path has more than one component,
                // meaning the user actually supplied a filename rather than an empty string
                // that resolved to a bare synthetic root like "/virtual".
                let has_real_filename = path
                    .parent()
                    .map(|p| p != std::path::Path::new("/") && p != std::path::Path::new(""))
                    .unwrap_or(false);
                if has_real_filename {
                    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !file_name.is_empty() {
                        return format!("{}: {}", prefix, file_name);
                    }
                }
                return prefix.to_string();
            }
        }
        message.to_string()
    }

    /// Check if this diagnostic indicates a file system access problem
    /// that might be caused by the flatpak sandbox
    pub fn is_file_access_error(&self) -> bool {
        matches!(
            self.kind,
            DiagnosticKind::FileNotFound
                | DiagnosticKind::FileAccessDenied
                | DiagnosticKind::FileIsDirectory
        )
    }

    fn compute_range(diag: &SourceDiagnostic, world: &TypesetterWorld) -> Option<(usize, usize)> {
        let file_id = diag.span.id()?;
        if file_id != world.main() {
            return None;
        }
        let text_range = world.range(diag.span)?;
        let source = world.source(file_id).ok()?;
        let text = source.text();

        let start = utils::byte_to_char_offset(text, text_range.start);
        let end = utils::byte_to_char_offset(text, text_range.end);
        Some((start, end))
    }

    fn compute_filename(diag: &SourceDiagnostic, world: &TypesetterWorld) -> Option<String> {
        let file_id = diag.span.id()?;
        if file_id != world.main() {
            Some(format!("{:?}", file_id))
        } else {
            None
        }
    }
}

impl std::fmt::Debug for PageImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageImage")
            .field("page_number", &self.page_number)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

#[derive(Debug)]
pub enum RenderResponse {
    PageImage(PageImage),
    Cancelled,
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfStandardType {
    V14,
    V15,
    V16,
    V17,
    V20,
    A1b,
    A1a,
    A2b,
    A2u,
    A2a,
    A3b,
    A3a,
    A3u,
    A4,
    A4f,
    A4e,
    Ua1,
}

impl PdfStandardType {
    pub fn to_pdf_standard(self) -> typst_pdf::PdfStandard {
        match self {
            Self::V14 => typst_pdf::PdfStandard::V_1_4,
            Self::V15 => typst_pdf::PdfStandard::V_1_5,
            Self::V16 => typst_pdf::PdfStandard::V_1_6,
            Self::V17 => typst_pdf::PdfStandard::V_1_7,
            Self::V20 => typst_pdf::PdfStandard::V_2_0,
            Self::A1b => typst_pdf::PdfStandard::A_1b,
            Self::A1a => typst_pdf::PdfStandard::A_1a,
            Self::A2b => typst_pdf::PdfStandard::A_2b,
            Self::A2u => typst_pdf::PdfStandard::A_2u,
            Self::A2a => typst_pdf::PdfStandard::A_2a,
            Self::A3b => typst_pdf::PdfStandard::A_3b,
            Self::A3a => typst_pdf::PdfStandard::A_3a,
            Self::A3u => typst_pdf::PdfStandard::A_3u,
            Self::A4 => typst_pdf::PdfStandard::A_4,
            Self::A4f => typst_pdf::PdfStandard::A_4f,
            Self::A4e => typst_pdf::PdfStandard::A_4e,
            Self::Ua1 => typst_pdf::PdfStandard::Ua_1,
        }
    }

    pub fn from_str(value: &str) -> Result<Self, CompilerError> {
        match value {
            "V14" => Ok(Self::V14),
            "V15" => Ok(Self::V15),
            "V16" => Ok(Self::V16),
            "V17" => Ok(Self::V17),
            "V20" => Ok(Self::V20),
            "A1b" => Ok(Self::A1b),
            "A1a" => Ok(Self::A1a),
            "A2b" => Ok(Self::A2b),
            "A2u" => Ok(Self::A2u),
            "A2a" => Ok(Self::A2a),
            "A3b" => Ok(Self::A3b),
            "A3a" => Ok(Self::A3a),
            "A3u" => Ok(Self::A3u),
            "A4" => Ok(Self::A4),
            "A4f" => Ok(Self::A4f),
            "A4e" => Ok(Self::A4e),
            "Ua1" => Ok(Self::Ua1),
            _ => Err(CompilerError::InvalidPdfStandard(value.to_string())),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::V14 => "V14",
            Self::V15 => "V15",
            Self::V16 => "V16",
            Self::V17 => "V17",
            Self::V20 => "V20",
            Self::A1b => "A1b",
            Self::A1a => "A1a",
            Self::A2b => "A2b",
            Self::A2u => "A2u",
            Self::A2a => "A2a",
            Self::A3b => "A3b",
            Self::A3a => "A3a",
            Self::A3u => "A3u",
            Self::A4 => "A4",
            Self::A4f => "A4f",
            Self::A4e => "A4e",
            Self::Ua1 => "Ua1",
        }
    }
}

/// Represents a location in the source code
#[derive(Debug, Clone)]
pub struct SourceLocation {
    pub offset: usize, // byte offset in the source
}

/// Represents a location in the rendered preview
#[derive(Debug, Clone)]
pub struct PreviewLocation {
    pub page_number: usize,
    pub x: f32, // position on page in points
    pub y: f32,
}

/// Result of a jump operation (either to source or preview)
#[derive(Debug, Clone)]
pub enum JumpResult {
    Source(SourceLocation),
    Preview(Vec<PreviewLocation>), // Can have multiple locations
    Url(String),
    None,
}

/// Document statistics
#[derive(Debug, Clone)]
pub struct DocumentStats {
    /// Number of words.
    pub words: usize,
    /// Number of characters.
    pub chars: usize,
    /// Number of spaces (space transitions).
    pub spaces: usize,
    /// Number of CJK characters.
    pub cjk_chars: usize,
    /// Number of pages in the document.
    pub pages: usize,
    /// Number of paragraphs in the document.
    pub paragraphs: usize,
    /// Number of headings in the document.
    pub headings: usize,
    /// Number of equations in the document.
    pub equations: usize,
}

#[derive(Debug, Clone)]
pub struct DocumentHeading {
    /// Plain-text label for display, with numbering prepended if present
    /// e.g. "1.2 Introduction" or just "Introduction"
    pub label: String,
    /// Resolved heading level (1 = top-level section, etc.)
    pub level: usize,
    /// Byte offset into the main source file, for jump_from_source
    pub source_offset: usize,
}

/// Tooltip content that can be displayed on hover
#[derive(Debug, Clone)]
pub enum TooltipContent {
    /// Plain text tooltip
    Text(String),
    /// Typst code tooltip
    Code(String),
}

impl From<IdeTooltip> for TooltipContent {
    fn from(tooltip: IdeTooltip) -> Self {
        match tooltip {
            IdeTooltip::Text(text) => TooltipContent::Text(text.to_string()),
            IdeTooltip::Code(code) => TooltipContent::Code(code.to_string()),
        }
    }
}

/// Result of a hover request
#[derive(Debug, Clone)]
pub struct HoverResult {
    /// The tooltip content to display
    pub content: TooltipContent,
    /// The range in the source that this tooltip applies to (start, end in char offsets)
    pub range: Option<(usize, usize)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_file_not_found_message() {
        // Flatpak sandbox path with a real filename
        let msg = "file not found (searched at /run/user/1000/doc/ab12cd/image.png)";
        assert_eq!(
            PreviewDiagnostic::sanitize_file_not_found_message(msg),
            "file not found: image.png"
        );

        // Virtual path with a real filename
        let msg = "file not found (searched at /virtual/image.png)";
        assert_eq!(
            PreviewDiagnostic::sanitize_file_not_found_message(msg),
            "file not found: image.png"
        );

        // Real filename without extension (#image("image"))
        let msg = "file not found (searched at /run/user/1000/doc/ab12cd/image)";
        assert_eq!(
            PreviewDiagnostic::sanitize_file_not_found_message(msg),
            "file not found: image"
        );

        // Null/empty source (#image("")) — bare synthetic root, no filename
        let msg = "file not found (searched at /virtual)";
        assert_eq!(
            PreviewDiagnostic::sanitize_file_not_found_message(msg),
            "file not found"
        );

        // No parens — unchanged
        let msg = "file not found";
        assert_eq!(
            PreviewDiagnostic::sanitize_file_not_found_message(msg),
            "file not found"
        );
    }

    // Test PdfStandardType conversions
    #[test]
    fn test_pdf_standard_from_str_valid() {
        assert_eq!(
            PdfStandardType::from_str("V14").unwrap(),
            PdfStandardType::V14
        );
        assert_eq!(
            PdfStandardType::from_str("V17").unwrap(),
            PdfStandardType::V17
        );
        assert_eq!(
            PdfStandardType::from_str("A2b").unwrap(),
            PdfStandardType::A2b
        );
        assert_eq!(
            PdfStandardType::from_str("A4f").unwrap(),
            PdfStandardType::A4f
        );
    }

    #[test]
    fn test_pdf_standard_from_str_invalid() {
        assert!(PdfStandardType::from_str("invalid").is_err());
        assert!(PdfStandardType::from_str("V18").is_err());
        assert!(PdfStandardType::from_str("v14").is_err()); // case sensitive
        assert!(PdfStandardType::from_str("").is_err());
    }

    #[test]
    fn test_pdf_standard_roundtrip() {
        // Test that from_str -> as_str gives back the same string
        let standards = [
            "V14", "V15", "V16", "V17", "V20", "A1b", "A1a", "A2b", "A2u", "A2a", "A3b", "A3a",
            "A3u", "A4", "A4f", "A4e", "Ua1",
        ];

        for &std_str in &standards {
            let parsed = PdfStandardType::from_str(std_str).unwrap();
            assert_eq!(parsed.as_str(), std_str);
        }
    }

    // Test diagnostic classification
    #[test]
    fn test_classify_file_not_found() {
        let kind = PreviewDiagnostic::classify_diagnostic("file not found", Severity::Error);
        assert_eq!(kind, DiagnosticKind::FileNotFound);

        let kind = PreviewDiagnostic::classify_diagnostic(
            "File Not Found: /path/to/file.typ",
            Severity::Error,
        );
        assert_eq!(kind, DiagnosticKind::FileNotFound);
    }

    #[test]
    fn test_classify_access_denied() {
        let kind = PreviewDiagnostic::classify_diagnostic(
            "access denied to /path/to/file",
            Severity::Error,
        );
        assert_eq!(kind, DiagnosticKind::FileAccessDenied);

        let kind = PreviewDiagnostic::classify_diagnostic("permission denied", Severity::Error);
        assert_eq!(kind, DiagnosticKind::FileAccessDenied);
    }

    #[test]
    fn test_classify_directory_error() {
        let kind = PreviewDiagnostic::classify_diagnostic("path is a directory", Severity::Error);
        assert_eq!(kind, DiagnosticKind::FileIsDirectory);
    }

    #[test]
    fn test_classify_package_error() {
        let kind = PreviewDiagnostic::classify_diagnostic(
            "package @preview/cool:1.0.0 not found",
            Severity::Error,
        );
        assert_eq!(kind, DiagnosticKind::PackageError);

        let kind = PreviewDiagnostic::classify_diagnostic(
            "failed to load package something",
            Severity::Error,
        );
        assert_eq!(kind, DiagnosticKind::PackageError);
    }

    #[test]
    fn test_classify_compilation_error() {
        let kind = PreviewDiagnostic::classify_diagnostic(
            "expected expression, found eof",
            Severity::Error,
        );
        assert_eq!(kind, DiagnosticKind::CompilationError);

        let kind =
            PreviewDiagnostic::classify_diagnostic("undefined variable 'foo'", Severity::Error);
        assert_eq!(kind, DiagnosticKind::CompilationError);
    }

    #[test]
    fn test_classify_warnings_are_other() {
        // Warnings should not be classified as specific error types
        let kind = PreviewDiagnostic::classify_diagnostic("file not found", Severity::Warning);
        assert_eq!(kind, DiagnosticKind::Other);
    }

    #[test]
    fn test_is_file_access_error() {
        let diag_not_found = PreviewDiagnostic {
            filename: None,
            hints: vec![],
            location_range: None,
            message: "test".to_string(),
            severity: Severity::Error,
            kind: DiagnosticKind::FileNotFound,
        };
        assert!(diag_not_found.is_file_access_error());

        let diag_compilation = PreviewDiagnostic {
            filename: None,
            hints: vec![],
            location_range: None,
            message: "test".to_string(),
            severity: Severity::Error,
            kind: DiagnosticKind::CompilationError,
        };
        assert!(!diag_compilation.is_file_access_error());

        let diag_package = PreviewDiagnostic {
            filename: None,
            hints: vec![],
            location_range: None,
            message: "package @preview/cool:1.0.0 not found".to_string(),
            severity: Severity::Error,
            kind: DiagnosticKind::PackageError,
        };
        assert!(!diag_package.is_file_access_error());
    }

    // Test CompilerError Display implementation
    #[test]
    fn test_compiler_error_display() {
        let err = CompilerError::SendError("test message".to_string());
        assert_eq!(format!("{}", err), "Send error: test message");

        let err = CompilerError::InvalidPdfStandard("V99".to_string());
        assert_eq!(format!("{}", err), "Invalid PDF standard: V99");
    }

    // Test basic struct construction
    #[test]
    fn test_page_metadata_creation() {
        let meta = PageMetadata {
            page_number: 0,
            dimensions: (595.0, 842.0), // A4 in points
        };
        assert_eq!(meta.page_number, 0);
        assert_eq!(meta.dimensions, (595.0, 842.0));
    }

    #[test]
    fn test_source_location_creation() {
        let loc = SourceLocation { offset: 42 };
        assert_eq!(loc.offset, 42);
    }

    #[test]
    fn test_preview_location_creation() {
        let loc = PreviewLocation {
            page_number: 1,
            x: 100.5,
            y: 200.5,
        };
        assert_eq!(loc.page_number, 1);
        assert_eq!(loc.x, 100.5);
        assert_eq!(loc.y, 200.5);
    }

    // Test JumpResult enum variants
    #[test]
    fn test_jump_result_variants() {
        let result = JumpResult::None;
        assert!(matches!(result, JumpResult::None));

        let result = JumpResult::Url("https://example.com".to_string());
        assert!(matches!(result, JumpResult::Url(_)));

        let result = JumpResult::Source(SourceLocation { offset: 10 });
        assert!(matches!(result, JumpResult::Source(_)));

        let result = JumpResult::Preview(vec![PreviewLocation {
            page_number: 0,
            x: 0.0,
            y: 0.0,
        }]);
        assert!(matches!(result, JumpResult::Preview(_)));
    }
}
