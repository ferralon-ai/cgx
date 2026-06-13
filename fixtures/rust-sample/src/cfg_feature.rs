// cgx-fixture: cfg-condition attributes
// Covers: #[cfg(feature="...")] -> cfg-condition attribute on nodes/edges,
//         #[cfg(target_os=...)] platform gates, cfg(test) guards

fn log_legacy(msg: &str) {
    eprintln!("[LEGACY] {}", msg);
}

fn new_auth_check(token: &str) -> bool {
    !token.is_empty()
}

fn legacy_auth_check(token: &str) -> bool {
    token == "secret"
}

/// Only compiled with feature "legacy-auth".
/// cfg-condition: feature = "legacy-auth" on this node and its call edges.
#[cfg(feature = "legacy-auth")]
pub fn authenticate_legacy(token: &str) -> bool {
    log_legacy("using legacy auth");
    legacy_auth_check(token)
}

/// Always present — calls legacy variant only when the feature is active.
pub fn authenticate(token: &str) -> bool {
    #[cfg(feature = "legacy-auth")]
    {
        return authenticate_legacy(token);  // cfg-condition on this edge
    }
    #[cfg(not(feature = "legacy-auth"))]
    {
        return new_auth_check(token);       // cfg-condition on this edge
    }
}

/// Platform-gated function — cfg-condition: target_os = "linux".
#[cfg(target_os = "linux")]
pub fn linux_only_init() {
    eprintln!("linux init");
}

/// cfg(test) — only present during test builds.
#[cfg(test)]
mod internal_tests {
    use super::*;

    fn assert_true(b: bool) {
        assert!(b);
    }

    #[test]
    fn test_auth_nonempty() {
        assert_true(authenticate("token123"));
    }
}
