//! A `relswap::Downloader` that reports bytes as they arrive.
//!
//! `relswap`'s own [`UreqDownloader`] returns the whole body in one call, which is correct but
//! silent: `rozi update --apply` fetched several megabytes and printed nothing until it finished.
//! The trait is the seam - it says nothing about how the body is read - so rozi supplies an
//! implementation that streams the response and calls a sink as it goes.
//!
//! [`UreqDownloader`]: relswap::UreqDownloader

use std::io::Read;
use std::time::Duration;

use relswap::{DownloadResponse, Downloader, ReleaseError, ReleaseResult};
use ureq::ResponseExt;
use ureq::tls::{RootCerts, TlsConfig};
use url::Url;

/// Matches `relswap`'s own redirect ceiling so a release that resolves for the engine also
/// resolves here.
const MAX_REDIRECTS: u32 = 8;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How much body to accumulate between sink calls.
///
/// Small enough that a slow link still animates, large enough that a fast one does not spend its
/// time formatting a row: at 64 KiB an 18 MB archive reports about 290 times.
const CHUNK: usize = 64 * 1024;

/// What a download has transferred so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub downloaded: u64,
    /// The `Content-Length`, when the server sent one. A chunked response has no total, so a sink
    /// must be able to render indeterminate progress rather than assuming a fraction exists.
    pub total: Option<u64>,
}

impl Progress {
    /// How far along this is, or `None` when the total is unknown or nonsensical.
    pub fn fraction(&self) -> Option<f64> {
        match self.total {
            Some(total) if total > 0 => Some(self.downloaded as f64 / total as f64),
            _ => None,
        }
    }
}

/// Receives progress while a body streams.
///
/// Implementations must tolerate being called often and out of proportion to real time; the
/// terminal sink throttles its own redraws rather than asking the transport to slow down.
pub trait ProgressSink {
    fn advance(&self, progress: Progress);
}

/// A sink that discards everything, for callers with nothing to draw on.
pub struct SilentSink;

impl ProgressSink for SilentSink {
    fn advance(&self, _progress: Progress) {}
}

/// Forward through a reference, so a caller can keep ownership of a sink it also needs to finish
/// afterwards. This is why the trait takes `&self`: a downloader holds the sink for the length of
/// the transfer, and the caller still has to erase the row when it ends.
impl<T: ProgressSink + ?Sized> ProgressSink for &T {
    fn advance(&self, progress: Progress) {
        (**self).advance(progress)
    }
}

/// A streaming downloader that reports progress to `sink`.
pub struct ProgressDownloader<S> {
    agent: ureq::Agent,
    sink: S,
    /// Bodies at or below this are fetched without reporting. Metadata and signature files are
    /// well under a kilobyte and would only flash a bar on and off.
    report_above: usize,
}

impl<S: ProgressSink> ProgressDownloader<S> {
    pub fn new(sink: S) -> Self {
        Self {
            agent: ureq::Agent::new_with_config(production_config()),
            sink,
            report_above: 1024 * 1024,
        }
    }

    /// Report on every response regardless of size. Test seam.
    #[cfg(test)]
    fn reporting_always(mut self) -> Self {
        self.report_above = 0;
        self
    }
}

/// The transport configuration, kept deliberately identical to `relswap`'s.
///
/// [`RootCerts::PlatformVerifier`] is the 0.0.4 fix and the reason this file cannot simply take
/// ureq's defaults: verifying against a compiled-in root snapshot is what made every managed
/// install fail with `UnknownIssuer` once GitHub's asset host moved to ISRG `Root YR`. The
/// bootstrap scripts use the host trust store, and this must agree with them.
fn production_config() -> ureq::config::Config {
    ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .https_only(true)
        .max_redirects(MAX_REDIRECTS)
        .max_redirects_will_error(true)
        .save_redirect_history(true)
        .timeout_global(Some(REQUEST_TIMEOUT))
        .timeout_connect(Some(REQUEST_TIMEOUT))
        .timeout_recv_response(Some(REQUEST_TIMEOUT))
        .timeout_recv_body(Some(REQUEST_TIMEOUT))
        .build()
}

impl<S: ProgressSink> Downloader for ProgressDownloader<S> {
    fn fetch(&self, url: &Url, max_bytes: usize) -> ReleaseResult<DownloadResponse> {
        require_https(url)?;
        let mut response = self
            .agent
            .get(url.as_str())
            .call()
            .map_err(|error| ReleaseError::Download(error.to_string()))?;

        let final_url = Url::parse(response.get_uri().to_string().as_str()).map_err(|error| {
            ReleaseError::Download(format!("invalid final response URL: {error}"))
        })?;
        // The redirect chain must be re-checked: an HTTPS request that lands on plain HTTP part way
        // through has still exposed the response, and the release resolver inspects this history.
        require_https(&final_url)?;
        let redirect_history = response
            .get_redirect_history()
            .unwrap_or(&[])
            .iter()
            .map(|uri| {
                Url::parse(uri.to_string().as_str()).map_err(|error| {
                    ReleaseError::Download(format!("invalid redirect URL: {error}"))
                })
            })
            .collect::<ReleaseResult<Vec<_>>>()?;

        let body = response.body_mut();
        let total = body.content_length();
        let report = total.is_none_or(|total| total > self.report_above as u64);

        // Read one byte past the ceiling so an oversized body is caught rather than silently
        // truncated to exactly the limit, which would then fail checksum verification instead and
        // report the wrong cause.
        let read_limit = max_bytes
            .checked_add(1)
            .ok_or_else(|| ReleaseError::Download("download size limit overflow".to_string()))?;
        let read_limit = u64::try_from(read_limit)
            .map_err(|_| ReleaseError::Download("download size limit exceeds u64".to_string()))?;

        let mut reader = body.with_config().limit(read_limit).reader();
        let mut bytes = Vec::with_capacity(
            total
                .and_then(|total| usize::try_from(total).ok())
                .unwrap_or(CHUNK)
                .min(max_bytes),
        );
        let mut chunk = vec![0u8; CHUNK];

        if report {
            self.sink.advance(Progress {
                downloaded: 0,
                total,
            });
        }
        loop {
            let read = reader
                .read(&mut chunk)
                .map_err(|error| ReleaseError::Download(error.to_string()))?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..read]);
            if report {
                self.sink.advance(Progress {
                    downloaded: bytes.len() as u64,
                    total,
                });
            }
        }

        if bytes.len() > max_bytes {
            return Err(ReleaseError::Download(format!(
                "response body exceeds maximum size {max_bytes}"
            )));
        }

        Ok(DownloadResponse::new(
            url.clone(),
            final_url,
            redirect_history,
            bytes,
        ))
    }
}

/// Reject anything that is not HTTPS before a request is made.
fn require_https(url: &Url) -> ReleaseResult<()> {
    if url.scheme() == "https" {
        Ok(())
    } else {
        Err(ReleaseError::Download(format!(
            "release URLs must use HTTPS, got {}",
            url.scheme()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<Progress>>);

    impl ProgressSink for Recorder {
        fn advance(&self, progress: Progress) {
            self.0.lock().expect("sink lock").push(progress);
        }
    }

    #[test]
    fn a_known_total_yields_a_fraction_and_an_unknown_one_does_not() {
        assert_eq!(
            Progress {
                downloaded: 5,
                total: Some(10)
            }
            .fraction(),
            Some(0.5)
        );
        // A chunked response has no Content-Length; the renderer must not divide by it.
        assert_eq!(
            Progress {
                downloaded: 5,
                total: None
            }
            .fraction(),
            None
        );
        // A server reporting zero length while sending a body must not produce infinity.
        assert_eq!(
            Progress {
                downloaded: 5,
                total: Some(0)
            }
            .fraction(),
            None
        );
    }

    #[test]
    fn plain_http_is_refused_before_any_request_is_made() {
        let url = Url::parse("http://example.invalid/rozi.tar.gz").expect("url");
        let error = require_https(&url).expect_err("http must be refused");
        assert!(error.to_string().contains("must use HTTPS"), "{error}");
    }

    #[test]
    fn https_is_accepted() {
        let url = Url::parse("https://example.invalid/rozi.tar.gz").expect("url");
        assert!(require_https(&url).is_ok());
    }

    #[test]
    fn the_streaming_downloader_keeps_the_platform_certificate_verifier() {
        // The 0.0.4 regression in one assertion: dropping to a compiled-in root snapshot here
        // would reintroduce `UnknownIssuer` on every managed install.
        let config = production_config();
        assert!(matches!(
            config.tls_config().root_certs(),
            RootCerts::PlatformVerifier
        ));
        assert!(config.https_only());
    }

    #[test]
    fn a_sink_can_be_passed_by_reference_and_still_be_owned_by_the_caller() {
        // The CLI needs this: it hands the downloader a borrow and keeps the row so it can erase
        // it once the transfer ends, whether that was a success or a failure.
        let recorder = Recorder::default();
        let borrowed: &Recorder = &recorder;
        borrowed.advance(Progress {
            downloaded: 1,
            total: Some(2),
        });
        assert_eq!(recorder.0.lock().expect("lock").len(), 1);
    }

    #[test]
    fn a_recorder_sink_collects_every_advance() {
        let recorder = Recorder::default();
        let downloader = ProgressDownloader::new(&recorder).reporting_always();
        assert_eq!(downloader.report_above, 0);
        downloader.sink.advance(Progress {
            downloaded: 3,
            total: Some(9),
        });
        assert_eq!(
            recorder.0.lock().expect("lock").as_slice(),
            &[Progress {
                downloaded: 3,
                total: Some(9)
            }]
        );
    }
}
