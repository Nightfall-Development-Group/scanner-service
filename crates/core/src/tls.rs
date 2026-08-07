//! TLS provider selection.
//!
//! rustls needs exactly one cryptographic backend, and reqwest's default choice
//! is `aws-lc-rs`, which is a C library requiring a working C cross-compiler and
//! NASM to build. That is fine when compiling natively but makes cross-building
//! and CI substantially more fragile — and we ship three platforms.
//!
//! `ring` does the same job with a far simpler build, so we take reqwest's
//! `rustls-no-provider` feature and install `ring` ourselves. That means the
//! provider must be registered once, before the first TLS connection.

use std::sync::Once;

static INIT: Once = Once::new();

/// Install `ring` as the process-wide rustls provider.
///
/// Idempotent and safe to call from anywhere that is about to make a request;
/// every entry point that builds an HTTP client calls it. Installing a provider
/// when one is already present is not an error we need to care about — it just
/// means someone got here first.
pub fn ensure_provider() {
    INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installing_twice_is_harmless() {
        // The guard exists because a second install returns Err, and any code
        // path that treated that as fatal would crash the app on a second
        // client being constructed.
        ensure_provider();
        ensure_provider();
        ensure_provider();
    }
}
