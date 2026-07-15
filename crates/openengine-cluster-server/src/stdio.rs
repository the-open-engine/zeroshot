use crate::{ClusterBackend, Dispatcher};
use tokio::io::{self, AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

pub async fn serve_ndjson<B, R, W, E>(
    dispatcher: &Dispatcher<B>,
    mut input: R,
    mut output: W,
    mut diagnostics: E,
) -> io::Result<()>
where
    B: ClusterBackend,
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let mut frame = Vec::new();
    loop {
        frame.clear();
        let bytes_read = input.read_until(b'\n', &mut frame).await?;
        if bytes_read == 0 {
            break;
        }
        trim_line_ending(&mut frame);
        serve_frame(dispatcher, &frame, &mut output, &mut diagnostics).await?;
    }
    Ok(())
}

fn trim_line_ending(frame: &mut Vec<u8>) {
    if frame.last() == Some(&b'\n') {
        frame.pop();
    }
    if frame.last() == Some(&b'\r') {
        frame.pop();
    }
}

async fn serve_frame<B, W, E>(
    dispatcher: &Dispatcher<B>,
    frame: &[u8],
    output: &mut W,
    diagnostics: &mut E,
) -> io::Result<()>
where
    B: ClusterBackend,
    W: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let outcome = dispatcher.dispatch_bytes(frame).await;
    output.write_all(outcome.response.as_bytes()).await?;
    output.write_all(b"\n").await?;
    output.flush().await?;
    if let Some(diagnostic) = outcome.diagnostic {
        diagnostics.write_all(diagnostic.as_bytes()).await?;
        diagnostics.write_all(b"\n").await?;
        diagnostics.flush().await?;
    }
    Ok(())
}
