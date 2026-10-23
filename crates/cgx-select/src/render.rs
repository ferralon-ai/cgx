//! Rendering a stored fqn into language-native surface syntax.
//!
//! The inverse direction of the tokenizers: where a [`tokenizer`](crate::tokenizer)
//! lexes a user's native-syntax string into abstract segments, [`render_fqn`] takes
//! the abstract segments of a stored [`NodeRecord.fqn`](cgx_core::node::NodeRecord)
//! and re-joins them in one language's native syntax. Both directions share the
//! family separator table ([`Family::native_separator`]), so a rendered fqn parses
//! back to the same segments it came from.
//!
//! The stored fqn is an abstract segment sequence; the delimiter in its string
//! encoding ([`FQN_SEGMENT_DELIM`]) is opaque and never surfaces. Every language —
//! including those whose native separator happens to match that delimiter — is
//! produced by splitting on the delimiter and re-joining with the family
//! separator, so there is no pass-through special case.

use cgx_core::FQN_SEGMENT_DELIM;

use crate::family::Family;

/// Render a stored fqn into the surface syntax native to `lang`.
///
/// The fqn is split into its abstract segments on the opaque internal
/// [`FQN_SEGMENT_DELIM`] and the segments are re-joined with the native separator
/// of the family that owns `lang`. A `lang` no family claims (e.g. C/C++) has no
/// native separator to render into, so the stored encoding is returned unchanged —
/// a stable, if internal-looking, fallback rather than a guess.
pub fn render_fqn(fqn: &str, lang: &str) -> String {
    match Family::from_lang_tag(lang) {
        Some(family) => render_with_family(fqn, family),
        None => fqn.to_string(),
    }
}

/// Render a node's stored fqn into native syntax, resolving the language from the
/// node's `lang` tag and falling back to its source `file` extension when the tag
/// is absent.
///
/// Dataflow value nodes are synthesized without a `lang` tag (it is not part of
/// their identity), so [`render_fqn`] alone would return their internal encoding
/// and leak the opaque segment delimiter. The owning language is still recoverable
/// from the file extension, so this is the function the CLI renders nodes through;
/// [`render_fqn`] remains the pure `(segments, lang)` primitive. A node with
/// neither signal (nor a family-owned file) keeps the stored encoding.
pub fn render_node_fqn(fqn: &str, lang: &str, file: &str) -> String {
    match Family::from_lang_tag(lang).or_else(|| Family::from_path(file)) {
        Some(family) => render_with_family(fqn, family),
        None => fqn.to_string(),
    }
}

/// Split a stored fqn on the opaque internal delimiter and re-join its abstract
/// segments with `family`'s native separator.
fn render_with_family(fqn: &str, family: Family) -> String {
    fqn.split(FQN_SEGMENT_DELIM)
        .collect::<Vec<&str>>()
        .join(family.native_separator())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_families_render_with_a_dot() {
        assert_eq!(render_fqn("com::example::Foo", "java"), "com.example.Foo");
        assert_eq!(render_fqn("pkg::mod::func", "python"), "pkg.mod.func");
        assert_eq!(render_fqn("app::Service", "typescript"), "app.Service");
    }

    #[test]
    fn go_renders_with_slashes_including_host_and_receiver() {
        // The host segment carries a literal `.`; it is one segment, untouched.
        assert_eq!(
            render_fqn("github.com::org::repo::Handler", "go"),
            "github.com/org/repo/Handler"
        );
        // A receiver segment is one segment and survives the round-trip intact.
        assert_eq!(render_fqn("app::(*Server)::Serve", "go"), "app/(*Server)/Serve");
    }

    #[test]
    fn rust_is_produced_by_rendering_not_passed_through() {
        // Byte-identical to the stored form, but produced by the same split/join
        // path as every other language — no pass-through branch.
        assert_eq!(render_fqn("crate::module::Type", "rust"), "crate::module::Type");
    }

    #[test]
    fn unknown_lang_falls_back_to_the_stored_encoding() {
        assert_eq!(render_fqn("a::b::c", "cpp"), "a::b::c");
        assert_eq!(render_fqn("a::b::c", ""), "a::b::c");
    }

    #[test]
    fn single_segment_is_separator_free() {
        assert_eq!(render_fqn("Open", "go"), "Open");
        assert_eq!(render_fqn("Open", "java"), "Open");
    }

    /// A node with an empty `lang` tag (a dataflow value node) still renders in its
    /// owning language's native syntax, recovered from the file extension — the
    /// internal delimiter must never leak just because the tag was synthesized away.
    #[test]
    fn empty_lang_falls_back_to_file_extension() {
        // Go value node: `::` would otherwise survive; the `.go` file rescues it.
        assert_eq!(
            render_node_fqn("go_sample::Transform::a#0", "", "dataflow.go"),
            "go_sample/Transform/a#0"
        );
        assert_eq!(
            render_node_fqn("dataflow::transform::b#1", "", "dataflow.py"),
            "dataflow.transform.b#1"
        );
        // An explicit tag always wins over the file extension.
        assert_eq!(
            render_node_fqn("a::b::c", "rust", "weird.py"),
            "a::b::c"
        );
        // No tag and no family-owned extension: stored encoding preserved.
        assert_eq!(render_node_fqn("a::b::c", "", "main.cpp"), "a::b::c");
        assert_eq!(render_node_fqn("a::b::c", "", "noext"), "a::b::c");
    }

    /// Rendering is segment-preserving: splitting the rendered string back on the
    /// family's native separator recovers exactly the original abstract segments.
    /// This is the render-direction round-trip — it shares the separator table with
    /// the tokenizers, so a rendered fqn re-segments identically. The Go cases are
    /// the load-bearing ones: a host segment carries a literal `.` and a receiver
    /// segment carries `(`/`*`/`)`, yet neither is fragmented by the `/` separator.
    #[test]
    fn render_then_resplit_recovers_segments() {
        let cases: &[(&str, &str)] = &[
            ("com::example::Service", "java"),
            ("pkg::mod::func", "python"),
            ("app::Service::method", "typescript"),
            ("crate::module::Type", "rust"),
            ("github.com::org::repo::Handler", "go"), // host segment carries a `.`
            ("app::(*Server)::Serve", "go"),          // receiver segment carries `(`/`*`/`)`
            ("Open", "go"),
            ("Open", "java"),
        ];
        for (fqn, lang) in cases {
            let sep = Family::from_lang_tag(lang).unwrap().native_separator();
            let rendered = render_fqn(fqn, lang);
            let recovered: Vec<&str> = rendered.split(sep).collect();
            let original: Vec<&str> = fqn.split(FQN_SEGMENT_DELIM).collect();
            assert_eq!(
                recovered, original,
                "re-splitting rendered {lang} fqn {rendered:?} must recover its segments"
            );
        }
    }
}
