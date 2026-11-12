// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use gtk::glib::Bytes;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use typst_pdf::{PdfOptions, PdfStandards};

use super::DocumentArena;
use super::types::{CompilerError, PdfStandardType, PreviewResult};

#[derive(Clone, Debug)]
pub struct CompilationWorker {
    cancellation_token: CancellationToken,
    sender: mpsc::Sender<CompilationMessage>,
}

#[allow(clippy::enum_variant_names)]
enum CompilationMessage {
    CompileForPreview {
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
        response: oneshot::Sender<Result<PreviewResult, CompilerError>>,
    },
    CompilePdf {
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
        pdf_standard: PdfStandardType,
        response: oneshot::Sender<Result<Option<Bytes>, CompilerError>>,
    },
    CompileHtml {
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
        response: oneshot::Sender<Result<Option<Bytes>, CompilerError>>,
    },
    CompileMarkdown {
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
        response: oneshot::Sender<Result<Option<String>, CompilerError>>,
    },
}

impl CompilationWorker {
    pub fn new(arena: Arc<DocumentArena>) -> Self {
        let cancellation_token = CancellationToken::new();
        let (sender, mut receiver) = mpsc::channel::<CompilationMessage>(16);

        let worker = Self {
            cancellation_token: cancellation_token.clone(),
            sender: sender.clone(),
        };

        let runtime = crate::application::TypesetterApplication::tokio_runtime();
        runtime.spawn(async move {
            let mut current_compilation: Option<tokio::task::JoinHandle<()>> = None;
            let mut queued_message: Option<CompilationMessage> = None;

            loop {
                tokio::select! {
                    biased;

                    msg = receiver.recv() => {
                        match msg {
                            Some(message) => {
                                let mut latest_message = message;

                                #[cfg(debug_assertions)]
                                let mut drained_count = 0;
                                while let Ok(newer) = receiver.try_recv() {
                                    #[cfg(debug_assertions)]
                                    {
                                        drained_count += 1;
                                    }
                                    latest_message = newer;
                                }

                                #[cfg(debug_assertions)]
                                if drained_count > 0 {
                                    println!("CompilationWorker: Drained {} stale messages", drained_count);
                                }

                                if current_compilation.is_some() {
                                    queued_message = Some(latest_message);
                                } else {
                                    current_compilation = Some(Self::spawn_compilation(
                                        Arc::clone(&arena),
                                        latest_message
                                    ));
                                }
                            }
                            None => {
                                #[cfg(debug_assertions)]
                                println!("CompilationWorker: Channel closed, stopping");
                                break;
                            }
                        }
                    }

                    Some(_) = async {
                        match &mut current_compilation {
                            Some(handle) => Some(handle.await),
                            None => None
                        }
                    } => {
                        current_compilation = None;

                        while let Ok(newer) = receiver.try_recv() {
                            queued_message = Some(newer);
                        }

                        if let Some(queued) = queued_message.take() {
                            current_compilation = Some(Self::spawn_compilation(
                                Arc::clone(&arena),
                                queued
                            ));
                        }
                    }

                    _ = cancellation_token.cancelled() => {
                        #[cfg(debug_assertions)]
                        println!("CompilationWorker: Cancellation requested, stopping");
                        break;
                    }
                }
            }
        });

        worker
    }

    pub async fn compile_for_preview(
        &self,
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
    ) -> Result<PreviewResult, CompilerError> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(CompilationMessage::CompileForPreview {
                source,
                path,
                project_root,
                response: tx,
            })
            .await
            .map_err(|e| CompilerError::SendError(e.to_string()))?;
        rx.await.map_err(|_| CompilerError::Cancelled)?
    }

    pub async fn compile_pdf(
        &self,
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
        pdf_standard: String,
    ) -> Result<Option<Bytes>, CompilerError> {
        let pdf_standard_type = PdfStandardType::from_str(&pdf_standard)?;
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(CompilationMessage::CompilePdf {
                source,
                path,
                project_root,
                pdf_standard: pdf_standard_type,
                response: tx,
            })
            .await
            .map_err(|e| CompilerError::SendError(e.to_string()))?;
        rx.await.map_err(|_| CompilerError::Cancelled)?
    }

    pub async fn compile_html(
        &self,
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
    ) -> Result<Option<Bytes>, CompilerError> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(CompilationMessage::CompileHtml {
                source,
                path,
                project_root,
                response: tx,
            })
            .await
            .map_err(|e| CompilerError::SendError(e.to_string()))?;
        rx.await.map_err(|_| CompilerError::Cancelled)?
    }

    pub async fn compile_markdown(
        &self,
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
    ) -> Result<Option<String>, CompilerError> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(CompilationMessage::CompileMarkdown {
                source,
                path,
                project_root,
                response: tx,
            })
            .await
            .map_err(|e| CompilerError::SendError(e.to_string()))?;
        rx.await.map_err(|_| CompilerError::Cancelled)?
    }

    pub fn cancel(&self) {
        self.cancellation_token.cancel();
    }

    fn spawn_compilation(
        arena: Arc<DocumentArena>,
        message: CompilationMessage,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::spawn_blocking(move || match message {
            CompilationMessage::CompileForPreview {
                source,
                path,
                project_root,
                response,
            } => {
                let result = arena.compile_and_update(source, path, project_root);
                let _ = response.send(Ok(result));
            }
            CompilationMessage::CompilePdf {
                source,
                path,
                project_root,
                pdf_standard,
                response,
            } => {
                let result =
                    Self::handle_compile_pdf(&arena, source, path, project_root, pdf_standard);
                let _ = response.send(result);
            }
            CompilationMessage::CompileHtml {
                source,
                path,
                project_root,
                response,
            } => {
                let result = Self::handle_compile_html(&arena, source, path, project_root);
                let _ = response.send(result);
            }
            CompilationMessage::CompileMarkdown {
                source,
                path,
                project_root,
                response,
            } => {
                let result = Self::handle_compile_markdown(&arena, source, path, project_root);
                let _ = response.send(result);
            }
        })
    }

    fn handle_compile_pdf(
        arena: &DocumentArena,
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
        pdf_standard: PdfStandardType,
    ) -> Result<Option<Bytes>, CompilerError> {
        // Compile and update arena
        let _result = arena.compile_and_update(source, path, project_root);

        let standard = pdf_standard.to_pdf_standard();
        let standards = PdfStandards::new(&[standard]).map_err(|e| {
            CompilerError::CompilationFailed(format!("Invalid PDF standard: {:?}", e))
        })?;
        let options = PdfOptions {
            standards,
            ..Default::default()
        };

        // Generate PDF from the document in arena
        arena.with_document(|_world, doc_opt| {
            match doc_opt {
                Some(doc) => {
                    match typst_pdf::pdf(doc, &options) {
                        Ok(bytes) => Ok(Some(Bytes::from(&bytes[..]))),
                        Err(errors) => {
                            // Extract human-readable error messages from diagnostics
                            let error_messages: Vec<String> =
                                errors.iter().map(|diag| diag.message.to_string()).collect();

                            let error_text = if error_messages.is_empty() {
                                "Unknown PDF error".to_string()
                            } else if error_messages.len() == 1 {
                                error_messages[0].clone()
                            } else {
                                error_messages.join("; ")
                            };

                            Err(CompilerError::CompilationFailed(error_text))
                        }
                    }
                }
                None => Ok(None),
            }
        })
    }

    fn handle_compile_html(
        arena: &DocumentArena,
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
    ) -> Result<Option<Bytes>, CompilerError> {
        arena.with_document(|world, _| {
            let mut html_world = world.clone();
            html_world.update(path.clone(), project_root.clone(), source.clone());

            let compilation = typst::compile::<typst_html::HtmlDocument>(&html_world);

            match compilation.output {
                Ok(html_doc) => {
                    match typst_html::html(&html_doc, &typst_html::HtmlOptions::default()) {
                        Ok(html_string) => Ok(Some(Bytes::from(html_string.as_bytes()))),
                        Err(errors) => {
                            let msg = errors
                                .iter()
                                .map(|e| e.message.to_string())
                                .collect::<Vec<_>>()
                                .join("; ");
                            Err(CompilerError::CompilationFailed(msg))
                        }
                    }
                }
                Err(_) => Ok(None),
            }
        })
    }

    fn handle_compile_markdown(
        arena: &DocumentArena,
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
    ) -> Result<Option<String>, CompilerError> {
        arena.with_document(|world, _| {
            let mut html_world = world.clone();
            html_world.update(path.clone(), project_root.clone(), source.clone());

            let compilation = typst::compile::<typst_html::HtmlDocument>(&html_world);

            match compilation.output {
                Ok(html_doc) => Ok(Some(super::markdown::document_to_markdown(&html_doc))),
                Err(_) => Ok(None),
            }
        })
    }
}
