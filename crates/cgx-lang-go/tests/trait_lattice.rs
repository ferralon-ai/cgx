mod common;

use cgx_frontend::RelationKind;
use common::extract;

#[test]
fn in_file_method_set_superset_emits_implements() {
    let src = "package svc\n\
               type Reader interface { Read() int }\n\
               type File struct{}\n\
               func (f File) Read() int { return 0 }\n";
    let facts = extract("app/svc.go", src);
    assert!(
        facts
            .impl_relations
            .iter()
            .any(|r| r.kind == RelationKind::Implements
                && r.subject.last().map(String::as_str) == Some("File")
                && r.object.last().map(String::as_str) == Some("Reader")),
        "got {:?}",
        facts.impl_relations
    );
}

#[test]
fn pointer_receiver_methods_count_toward_implements() {
    let src = "package svc\n\
               type Reader interface { Read() int }\n\
               type File struct{}\n\
               func (f *File) Read() int { return 0 }\n";
    let facts = extract("app/svc.go", src);
    assert!(
        facts
            .impl_relations
            .iter()
            .any(|r| r.kind == RelationKind::Implements
                && r.subject.last().map(String::as_str) == Some("File")),
        "got {:?}",
        facts.impl_relations
    );
}

#[test]
fn partial_method_set_does_not_emit_implements() {
    let src = "package svc\n\
               type RW interface { Read() int; Write() int }\n\
               type File struct{}\n\
               func (f File) Read() int { return 0 }\n";
    let facts = extract("app/svc.go", src);
    assert!(!facts
        .impl_relations
        .iter()
        .any(|r| r.kind == RelationKind::Implements));
}
