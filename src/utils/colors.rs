// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Color transformation utilities for preview rendering.
//!
//! Operates on raw R8G8B8A8 pixel buffers in-place with no allocation overhead.

// Each matrice is a 3x3 row-major matrix applied to [R, G, B].
pub type Matrix = [[f32; 3]; 3];
// An [R, G, B] offset added after the matrix multiply (alpha is never
// offset - see graphene_color_matrix in image.rs/magnifier.rs).
pub type Offset = [f32; 3];

const IDENTITY: Matrix = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// GPU approximation of "invert lightness, preserve hue/saturation":
/// invert luma in YIQ space instead of lightness in HSL space. RGB->YIQ,
/// negate+offset Y only (keep I/Q untouched), YIQ->RGB. Three linear
/// steps, and a composition of affine transforms is itself affine, so the
/// whole thing collapses into this one matrix + offset.
const INVERT_LUMA_MATRIX: Matrix = [
    [0.401807, -1.173727, -0.228080],
    [-0.597629, -0.174091, -0.228280],
    [-0.598843, -1.174625, 0.773468],
];
const INVERT_LUMA_OFFSET: Offset = [1.0, 1.0, 1.0];

/// Combines the colorblind matrix (if any) and the luma-invert transform
/// (if enabled) into a single (matrix, offset) affine transform, so GPU
/// callers only ever need one push_color_matrix() for both effects
/// together instead of nesting two. Colorblind is applied first, matching
/// the order the original CPU pipeline used (matrix mix, then invert).
/// Returns None if neither effect is active.
pub fn combined_transform(invert: bool, cb_mode: &str) -> Option<(Matrix, Offset)> {
    let cb = cb_matrix(cb_mode);

    if !invert {
        return cb.map(|m| (m, [0.0, 0.0, 0.0]));
    }

    let m1 = cb.unwrap_or(IDENTITY);
    let m2 = INVERT_LUMA_MATRIX;
    let o2 = INVERT_LUMA_OFFSET;

    // composed(x) = m2 * (m1 * x + 0) + o2 = (m2 * m1) * x + o2
    // (o1 is always zero here - cb_matrix never has an offset - so the
    // general "o = m2*o1 + o2" collapses to just o2)
    Some((matmul(m2, m1), o2))
}

fn matmul(a: Matrix, b: Matrix) -> Matrix {
    let mut r = [[0f32; 3]; 3];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

/// Returns the colorblindness-simulation matrix for a given mode name, or
/// `None` for "none"/unrecognized modes. Callers doing GPU rendering (see
/// `PreviewImage::snapshot()`) embed this directly into a 4x4 color matrix
/// for `gtk_snapshot_push_color_matrix()`; it's exposed here rather than
/// re-implemented at the call site so there's exactly one place that owns
/// what each simulation mode actually looks like.
pub fn cb_matrix(mode: &str) -> Option<Matrix> {
    match mode {
        "deuteranopia" => Some(DEUTERANOPIA),
        "protanopia" => Some(PROTANOPIA),
        "tritanopia" => Some(TRITANOPIA),
        "achromatopsia" => Some(ACHROMATOPSIA),
        _ => None,
    }
}

const DEUTERANOPIA: Matrix = [
    [0.367322, 0.860646, -0.227968],
    [0.280085, 0.672501, 0.047413],
    [-0.011820, 0.042940, 0.968881],
];

const PROTANOPIA: Matrix = [
    [0.152286, 1.052583, -0.204868],
    [0.114503, 0.786281, 0.099216],
    [-0.003882, -0.048116, 1.051998],
];

const TRITANOPIA: Matrix = [
    [1.255528, -0.076749, -0.178779],
    [-0.078411, 0.930809, 0.147602],
    [0.004733, 0.691367, 0.303900],
];

const ACHROMATOPSIA: Matrix = [
    [0.299, 0.587, 0.114],
    [0.299, 0.587, 0.114],
    [0.299, 0.587, 0.114],
];
