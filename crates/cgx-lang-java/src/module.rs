//! Package-prefix (FQN root) derivation for Java.
//!
//! Unlike Go (where the package maps to the file's directory path), a Java
//! file's package is declared in source by a `package a.b.c;` statement. The
//! extractor reads that `package_declaration` node and passes the dotted name
//! here; the file path is never consulted. A file with no package declaration
//! lives in the *default package*, whose prefix is empty (top-level types are
//! then FQN'd by their bare class name).

/// `::`-joined FQN prefix from a dotted Java package name (`"com.example.app"`
/// → `"com::example::app"`). An empty or whitespace-only input yields `""`
/// (the default package).
pub fn module_path_from_package(pkg_decl: &str) -> String {
    pkg_decl
        .split('.')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("::")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_package_becomes_colon_path() {
        assert_eq!(module_path_from_package("com.example.app"), "com::example::app");
    }

    #[test]
    fn single_segment_package() {
        assert_eq!(module_path_from_package("app"), "app");
    }

    #[test]
    fn default_package_is_empty_prefix() {
        assert_eq!(module_path_from_package(""), "");
        assert_eq!(module_path_from_package("   "), "");
    }
}
