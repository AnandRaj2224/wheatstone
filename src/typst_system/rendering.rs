// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use typst::utils::Scalar;
use typst_render::{RenderOptions, render};

use super::DocumentArena;
use super::types::{PageImage, RenderResponse};

#[derive(Clone, Debug)]
pub struct RenderingWorker {
    cancellation_token: CancellationToken,
    sender: mpsc::Sender<RenderingMessage>,
}

enum RenderingMessage {
    RenderPage {
        generation: u64,
        page_number: usize,
        pixel_per_pt: f64,
        response: oneshot::Sender<RenderResponse>,
    },
}

impl RenderingWorker {
    pub fn new(arena: Arc<DocumentArena>) -> Self {
        let cancellation_token = CancellationToken::new();
        let (sender, mut receiver) = mpsc::channel::<RenderingMessage>(32);

        let worker = Self {
            cancellation_token: cancellation_token.clone(),
            sender: sender.clone(),
        };

        let runtime = crate::application::TypesetterApplication::tokio_runtime();
        let semaphore = Arc::new(tokio::sync::Semaphore::new(2));

        runtime.spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        #[cfg(debug_assertions)]
                        println!("RenderingWorker: Cancellation requested, stopping");
                        break;
                    }
                    msg = receiver.recv() => {
                        match msg {
                            Some(RenderingMessage::RenderPage { generation, page_number, pixel_per_pt, response }) => {
                                // Fast-path: discard immediately if arena generation already advanced
                                if arena.current_generation() != generation {
                                    let _ = response.send(RenderResponse::Cancelled);
                                    continue;
                                }

                                let arena = Arc::clone(&arena);
                                let sem = Arc::clone(&semaphore);
                                tokio::spawn(async move {
                                    let Ok(permit) = sem.acquire_owned().await else {
                                        let _ = response.send(RenderResponse::Cancelled);
                                        return;
                                    };
                                    // Pre-render check: discard before blocking task if generation advanced while queued
                                    if arena.current_generation() != generation {
                                        drop(permit);
                                        let _ = response.send(RenderResponse::Cancelled);
                                        return;
                                    }
                                    let arena_blocking = Arc::clone(&arena);
                                    let result = tokio::task::spawn_blocking(move || {
                                        let _permit = permit;
                                        Self::handle_render_page(&arena_blocking, generation, page_number, pixel_per_pt)
                                    }).await.unwrap_or(RenderResponse::Cancelled);

                                    // Post-render check: if generation advanced while render was running, discard
                                    if arena.current_generation() != generation {
                                        let _ = response.send(RenderResponse::Cancelled);
                                        return;
                                    }
                                    let _ = response.send(result);
                                });
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        worker
    }

    /// Renders `page_number` at the given `pixel_per_pt` scale for `generation`.
    pub async fn render_page(&self, generation: u64, page_number: usize, pixel_per_pt: f64) -> RenderResponse {
        let (tx, rx) = oneshot::channel();
        if self
            .sender
            .send(RenderingMessage::RenderPage {
                generation,
                page_number,
                pixel_per_pt,
                response: tx,
            })
            .await
            .is_err()
        {
            return RenderResponse::Error("Render worker unavailable".to_string());
        }

        rx.await
            .unwrap_or(RenderResponse::Cancelled)
    }

    pub fn cancel(&self) {
        self.cancellation_token.cancel();
    }

    fn handle_render_page(
        arena: &DocumentArena,
        generation: u64,
        page_number: usize,
        pixel_per_pt: f64,
    ) -> RenderResponse {
        // Borrow document from arena with read lock
        arena.with_document(|_world, doc_opt| {
            if arena.current_generation() != generation {
                return RenderResponse::Cancelled;
            }

            let Some(doc) = doc_opt else {
                return RenderResponse::Error("No document available".to_string());
            };

            let Some(page) = doc.pages().get(page_number) else {
                return RenderResponse::Error(format!("Page {} not found", page_number));
            };

            #[cfg(debug_assertions)]
            {
                let page_width_pt = page.frame.width().to_pt();
                let page_height_pt = page.frame.height().to_pt();
                println!(
                    "RenderingWorker: Rendering page {} at pixel_per_pt={:.4} (page={:.1}x{:.1}pt -> expect ~{:.1}x{:.1}px)",
                    page_number,
                    pixel_per_pt,
                    page_width_pt,
                    page_height_pt,
                    page_width_pt * pixel_per_pt,
                    page_height_pt * pixel_per_pt,
                );
            }

            // Render with the caller's exact scale. Both width and height
            // are derived by typst_render from this single ratio, rather
            // than us pre-computing a rounded target width and having
            // typst_render re-derive an approximate ratio from it.
            let pixel_map = render(
                page,
                &RenderOptions {
                    pixel_per_pt: Scalar::new(pixel_per_pt),
                    render_bleed: false,
                },
            );

            // Post-render check: if generation advanced during the CPU-bound Typst render,
            // drop pixel_map immediately without allocating or copying pixel bytes
            if arena.current_generation() != generation {
                drop(pixel_map);
                return RenderResponse::Cancelled;
            }

            let pixels = pixel_map.data().to_vec();
            let width = pixel_map.width() as i32;
            let height = pixel_map.height() as i32;

            drop(pixel_map);

            RenderResponse::PageImage(PageImage {
                page_number,
                width,
                height,
                pixels,
            })
        })
    }
}
