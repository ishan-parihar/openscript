use tokio::sync::mpsc;
use std::sync::OnceLock;

/// Global channel for transcription progress notifications.
/// progress_pct ranges from 0.0 to 100.0.
static PROGRESS_TX: OnceLock<mpsc::UnboundedSender<f64>> = OnceLock::new();

/// Get or initialize the progress sender. Returns a channel that's always available.
fn get_tx() -> &'static mpsc::UnboundedSender<f64> {
    PROGRESS_TX.get_or_init(|| {
        let (tx, _rx) = mpsc::unbounded_channel();
        tx
    })
}

/// Report transcription progress (0.0 to 100.0).
/// Best-effort: if no receiver is listening, it's a no-op.
pub async fn report_transcribe_progress(progress_pct: f64) {
    let _ = get_tx().send(progress_pct);
}
