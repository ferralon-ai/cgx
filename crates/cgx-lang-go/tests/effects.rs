mod common;

use cgx_core::effect::Effect;
use common::extract;

fn effects_of(facts: &cgx_frontend::FileFacts, fqn_suffix: &str) -> cgx_core::effect::EffectSet {
    facts
        .effects
        .iter()
        .find(|e| e.fqn.ends_with(fqn_suffix))
        .map(|e| e.effects)
        .unwrap_or_default()
}

#[test]
fn http_call_is_io_net() {
    let facts = extract(
        "app/svc.go",
        "package svc\nimport \"net/http\"\nfunc F(){ http.Get(\"x\") }\n",
    );
    assert!(effects_of(&facts, "::F").contains(Effect::IoNet));
}

#[test]
fn time_now_is_nondeterministic() {
    let facts = extract(
        "app/svc.go",
        "package svc\nimport \"time\"\nfunc F(){ _ = time.Now() }\n",
    );
    assert!(effects_of(&facts, "::F").contains(Effect::Nondeterministic));
}

#[test]
fn exec_command_is_io_proc() {
    let facts = extract(
        "app/svc.go",
        "package svc\nimport \"os/exec\"\nfunc F(){ _ = exec.Command(\"ls\") }\n",
    );
    assert!(effects_of(&facts, "::F").contains(Effect::IoProc));
}

#[test]
fn mutex_lock_is_blocking() {
    let facts = extract(
        "app/svc.go",
        "package svc\nimport \"sync\"\nfunc F(mu *sync.Mutex){ mu.Lock() }\n",
    );
    assert!(effects_of(&facts, "::F").contains(Effect::Blocking));
}

#[test]
fn go_statement_spawns() {
    let facts = extract(
        "app/svc.go",
        "package svc\nfunc F(){ go worker() }\nfunc worker(){}\n",
    );
    assert!(effects_of(&facts, "::F").contains(Effect::Spawns));
}

#[test]
fn pure_func_has_no_effects() {
    let facts = extract("app/svc.go", "package svc\nfunc F(a int) int { return a + 1 }\n");
    assert!(effects_of(&facts, "::F").is_empty());
}
