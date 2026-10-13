//! Shared, bounded TypeSafe transport. Credentials never enter argv or files.
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const MODEL: &str = "jev-1.13.0";
pub(crate) const THRESHOLD: f64 = 0.95;
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) fn confident(confidence: f64, probability: f64, sufficiency: f64) -> bool {
    [confidence, probability, sufficiency]
        .into_iter()
        .all(|score| score.is_finite() && (THRESHOLD..=1.0).contains(&score))
}

// Messages are controlled by the adapter, never copied from provider bodies,
// curl stderr, request headers, or credentials.
#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) kind: &'static str,
    message: &'static str,
    http_status: Option<u16>,
}

impl Failure {
    pub(crate) fn new(kind: &'static str, message: &'static str) -> Self {
        Self {
            kind,
            message,
            http_status: None,
        }
    }
    pub(crate) fn evidence(&self) -> Value {
        json!({"kind": self.kind, "message": self.message, "http_status": self.http_status})
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Jev {}: {}", self.kind, self.message)?;
        if let Some(status) = self.http_status {
            write!(f, " (HTTP {status})")?;
        }
        Ok(())
    }
}
impl std::error::Error for Failure {}

pub(crate) fn evaluate(
    request: &Value,
    timeout: Duration,
    max_response_bytes: usize,
) -> std::result::Result<Value, Failure> {
    let budget = timeout.min(HTTP_TIMEOUT);
    if budget.is_zero() {
        return Err(Failure::new("timeout", "Invocation deadline is exhausted"));
    }
    let started = Instant::now();
    let key = std::env::var("TYPESAFE_API_KEY")
        .map_err(|_| Failure::new("configuration", "TYPESAFE_API_KEY is not set"))?;
    if key.trim().is_empty() || key.contains(['\r', '\n', '\0']) {
        return Err(Failure::new(
            "configuration",
            "TYPESAFE_API_KEY is empty or invalid",
        ));
    }
    // Headers and body travel over stdin, not process arguments or files.
    let config = format!("url = \"https://api.typesafe.ai/v1/systemone\"\nheader = {}\nheader = \"Content-Type: application/json\"\ndata = {}\n",
        json!(format!("Authorization: Bearer {key}")), json!(request.to_string()));
    let mut child = Command::new("curl")
        .args([
            "-q",
            "--silent",
            "--show-error",
            "--connect-timeout",
            "5",
            "--max-time",
            &budget.as_secs_f64().to_string(),
            "--write-out",
            "\n%{http_code}",
            "--config",
            "-",
        ])
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| {
            Failure::new(
                "configuration",
                "Could not launch curl; ensure it is installed",
            )
        })?;
    let pid = child.id();
    let mut stdin = child.stdin.take().expect("piped curl stdin");
    let stdout = child.stdout.take().expect("piped curl stdout");
    let (written_tx, written_rx) = mpsc::channel();
    let writer = thread::spawn(move || {
        let _ = written_tx.send(stdin.write_all(config.as_bytes()));
    });
    let (body_tx, body_rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut body = Vec::new();
        let result = stdout
            .take((max_response_bytes + 1) as u64)
            .read_to_end(&mut body);
        let _ = body_tx.send(result.map(|_| body));
    });
    let mut status = None;
    let mut body = None;
    let mut written = false;
    let transport = (|| loop {
        if started.elapsed() >= budget {
            return Err(Failure::new("timeout", "Jev request exceeded its deadline"));
        }
        if !written {
            match written_rx.try_recv() {
                Ok(Ok(())) => written = true,
                Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(Failure::new("transport", "Could not send the Jev request"))
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if body.is_none() {
            match body_rx.try_recv() {
                Ok(Ok(bytes)) if bytes.len() > max_response_bytes => {
                    return Err(Failure::new(
                        "invalid_response",
                        "Jev response exceeds the size limit",
                    ))
                }
                Ok(Ok(bytes)) => body = Some(bytes),
                Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(Failure::new("transport", "Could not read the Jev response"))
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if status.is_none() {
            status = crate::process::child_exited(&child)
                .map_err(|_| Failure::new("transport", "Could not inspect the HTTP process"))?;
        }
        if let (Some(exit), Some(body)) = (status, body.as_ref()) {
            if written {
                if !exit {
                    return Err(Failure::new("transport", "Jev HTTP transport failed"));
                }
                return decode_http(body);
            }
        }
        thread::sleep(Duration::from_millis(10));
    })();
    if transport.is_err() {
        // Only this invocation's owned process group is terminated. Readers
        // and writers must not outlive the deadline because a child held a pipe.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        let _ = child.kill();
    }
    // The non-reaping observation keeps the PID reserved until cleanup is done.
    let exit = child.wait();
    let _ = writer.join();
    let _ = reader.join();
    if exit.ok().and_then(|status| status.code()) == Some(28) {
        return Err(Failure::new("timeout", "Jev request exceeded its deadline"));
    }
    transport
}

fn decode_http(bytes: &[u8]) -> std::result::Result<Value, Failure> {
    let split = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .ok_or_else(|| Failure::new("transport", "HTTP status was not reported"))?;
    let status: u16 = std::str::from_utf8(&bytes[split + 1..])
        .ok()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| Failure::new("transport", "HTTP status was not reported"))?;
    if !(200..300).contains(&status) {
        let (kind, message) = match status {
            401 | 403 => ("authentication", "Jev rejected the API credentials"),
            429 => ("rate_limit", "Jev rate limit exceeded"),
            500..=599 => ("service", "Jev service is unavailable"),
            _ => ("http", "Jev rejected the request"),
        };
        return Err(Failure {
            kind,
            message,
            http_status: Some(status),
        });
    }
    serde_json::from_slice(&bytes[..split])
        .map_err(|_| Failure::new("invalid_response", "Jev response is not valid JSON"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failures_have_safe_typed_evidence_and_policy_is_explicit() {
        for (status, kind) in [
            (401, "authentication"),
            (429, "rate_limit"),
            (503, "service"),
            (400, "http"),
        ] {
            let bytes = format!("sensitive-provider-body\n{status}");
            let error = decode_http(bytes.as_bytes()).unwrap_err();
            assert_eq!(error.kind, kind);
            assert_eq!(error.http_status, Some(status));
            assert!(!error.to_string().contains("sensitive-provider-body"));
            assert_eq!(error.evidence()["kind"], kind);
        }
        assert_eq!(
            decode_http(b"not json\n200").unwrap_err().kind,
            "invalid_response"
        );
        assert_eq!(decode_http(b"no status").unwrap_err().kind, "transport");
    }

    #[test]
    fn zero_deadline_does_not_launch_http() {
        let start = Instant::now();
        let error = evaluate(&json!({}), Duration::ZERO, 1024).unwrap_err();
        assert_eq!(error.kind, "timeout");
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn confidence_boundary_is_inclusive_and_requires_all_scores() {
        assert!(confident(0.95, 0.95, 0.95));
        for scores in [
            (0.949999, 1.0, 1.0),
            (1.0, 0.949999, 1.0),
            (1.0, 1.0, 0.949999),
            (f64::NAN, 1.0, 1.0),
            (1.01, 1.0, 1.0),
        ] {
            assert!(!confident(scores.0, scores.1, scores.2));
        }
    }
}
