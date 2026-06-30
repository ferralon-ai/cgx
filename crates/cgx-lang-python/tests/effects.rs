//! Cycle-4 own-effects + concurrency-hint coverage for the Python adapter.
//!
//! Effect facts mirror Go's `effects` test: a name-based stdlib heuristic keyed by
//! the enclosing function FQN. Concurrency hints reuse the existing effect/ref
//! taxonomy (no new fact type): `spawns` effect + `Spawn` refs (LS-7.1), `blocking`
//! effect for lock `with` blocks (LS-7.3), and `CallAsync` refs for `await`
//! (LS-7.2) — exactly the kinds Go/Rust emit.

mod common;

use cgx_core::condition::EdgeCondition;
use cgx_core::effect::{Effect, EffectSet};
use cgx_frontend::RefKind;
use common::{extract, has_ref};

fn effects_of(facts: &cgx_frontend::FileFacts, fqn_suffix: &str) -> EffectSet {
    facts
        .effects
        .iter()
        .find(|e| e.fqn.ends_with(fqn_suffix))
        .map(|e| e.effects)
        .unwrap_or_default()
}

// --- own-effects (LS-7.4 + io.* surfaces) ---

#[test]
fn open_is_io_file() {
    let facts = extract("m.py", "def f(p):\n    return open(p)\n");
    assert!(effects_of(&facts, "::f").contains(Effect::IoFile));
}

#[test]
fn pathlib_read_text_is_io_file() {
    let facts = extract("m.py", "def f(p):\n    return p.read_text()\n");
    assert!(effects_of(&facts, "::f").contains(Effect::IoFile));
}

#[test]
fn shutil_copy_is_io_file() {
    let facts = extract("m.py", "import shutil\ndef f(a, b):\n    shutil.copy(a, b)\n");
    assert!(effects_of(&facts, "::f").contains(Effect::IoFile));
}

#[test]
fn requests_get_is_io_net() {
    let facts = extract("m.py", "import requests\ndef f(u):\n    return requests.get(u)\n");
    assert!(effects_of(&facts, "::f").contains(Effect::IoNet));
}

#[test]
fn urlopen_is_io_net() {
    let facts = extract(
        "m.py",
        "import urllib.request\ndef f(u):\n    return urllib.request.urlopen(u)\n",
    );
    assert!(effects_of(&facts, "::f").contains(Effect::IoNet));
}

#[test]
fn subprocess_run_is_io_proc() {
    let facts = extract("m.py", "import subprocess\ndef f():\n    subprocess.run(['ls'])\n");
    assert!(effects_of(&facts, "::f").contains(Effect::IoProc));
}

#[test]
fn os_system_is_io_proc() {
    let facts = extract("m.py", "import os\ndef f():\n    os.system('ls')\n");
    assert!(effects_of(&facts, "::f").contains(Effect::IoProc));
}

#[test]
fn eval_is_dynamic_code() {
    let facts = extract("m.py", "def f(s):\n    return eval(s)\n");
    assert!(effects_of(&facts, "::f").contains(Effect::DynamicCode));
}

#[test]
fn getenv_is_nondeterministic() {
    let facts = extract("m.py", "import os\ndef f():\n    return os.getenv('X')\n");
    assert!(effects_of(&facts, "::f").contains(Effect::Nondeterministic));
}

#[test]
fn random_is_nondeterministic() {
    let facts = extract("m.py", "import random\ndef f():\n    return random.random()\n");
    assert!(effects_of(&facts, "::f").contains(Effect::Nondeterministic));
}

#[test]
fn time_sleep_is_blocking() {
    let facts = extract("m.py", "import time\ndef f():\n    time.sleep(1)\n");
    assert!(effects_of(&facts, "::f").contains(Effect::Blocking));
}

#[test]
fn pure_func_has_no_effects() {
    let facts = extract("m.py", "def f(a):\n    return a + 1\n");
    assert!(effects_of(&facts, "::f").is_empty());
}

#[test]
fn effects_attribute_to_the_enclosing_method() {
    let facts = extract(
        "m.py",
        "import os\nclass C:\n    def m(self):\n        return os.getenv('X')\n",
    );
    assert!(effects_of(&facts, "C::m").contains(Effect::Nondeterministic));
}

// --- concurrency: spawn (LS-7.1) ---

#[test]
fn asyncio_create_task_spawns() {
    let facts = extract("m.py", "import asyncio\ndef f(c):\n    asyncio.create_task(c)\n");
    assert!(effects_of(&facts, "::f").contains(Effect::Spawns));
    assert!(has_ref(
        &facts,
        "create_task",
        RefKind::Spawn,
        EdgeCondition::Always
    ));
}

#[test]
fn asyncio_gather_spawns() {
    let facts = extract("m.py", "import asyncio\ndef f(a, b):\n    asyncio.gather(a, b)\n");
    assert!(effects_of(&facts, "::f").contains(Effect::Spawns));
}

#[test]
fn threading_thread_spawns() {
    let facts = extract(
        "m.py",
        "import threading\ndef f():\n    threading.Thread(target=worker)\n",
    );
    assert!(effects_of(&facts, "::f").contains(Effect::Spawns));
    assert!(has_ref(&facts, "Thread", RefKind::Spawn, EdgeCondition::Always));
}

#[test]
fn multiprocessing_process_spawns() {
    let facts = extract(
        "m.py",
        "import multiprocessing\ndef f():\n    multiprocessing.Process(target=w)\n",
    );
    assert!(effects_of(&facts, "::f").contains(Effect::Spawns));
}

#[test]
fn executor_submit_spawns() {
    let facts = extract("m.py", "def f(ex, fn):\n    ex.submit(fn)\n");
    assert!(effects_of(&facts, "::f").contains(Effect::Spawns));
    assert!(has_ref(&facts, "submit", RefKind::Spawn, EdgeCondition::Always));
}

// --- concurrency: lock (LS-7.3) ---

#[test]
fn with_lock_constructor_is_blocking() {
    let facts = extract(
        "m.py",
        "import threading\ndef f():\n    with threading.Lock():\n        pass\n",
    );
    assert!(effects_of(&facts, "::f").contains(Effect::Blocking));
}

#[test]
fn with_lock_name_is_blocking() {
    let facts = extract("m.py", "def f(lock):\n    with lock:\n        pass\n");
    assert!(effects_of(&facts, "::f").contains(Effect::Blocking));
}

#[test]
fn with_plain_resource_is_not_a_lock() {
    let facts = extract("m.py", "def f(p):\n    with open(p) as fh:\n        pass\n");
    let e = effects_of(&facts, "::f");
    assert!(!e.contains(Effect::Blocking));
    // `open(...)` inside the with-item is still an io.file call site.
    assert!(e.contains(Effect::IoFile));
}

// --- concurrency: async suspension (LS-7.2) ---

#[test]
fn await_call_is_call_async() {
    let facts = extract(
        "m.py",
        "async def f(u):\n    data = await fetch(u)\n    return data\n",
    );
    assert!(has_ref(&facts, "fetch", RefKind::CallAsync, EdgeCondition::Always));
}

#[test]
fn await_propagates_callee_effects() {
    let facts = extract(
        "m.py",
        "import asyncio\nasync def f():\n    await asyncio.sleep(1)\n",
    );
    // `asyncio.sleep` is a blocking yield point per the effect table.
    assert!(effects_of(&facts, "::f").contains(Effect::Blocking));
}
