//! The little language knowledge the receiver resolver needs, as tables.

/// Per-language rules for receiver narrowing.
#[derive(Debug)]
pub(crate) struct LangRules {
    /// Names that resolve to an out-of-repo class with no import (Python
    /// builtins). A base or class head with one of these names is `External`.
    pub builtin_types: &'static [&'static str],
    /// Methods every class inherits from the language root (`object`). A
    /// lookup on a class with no declared base finds them out of repo.
    pub implicit_root_methods: &'static [&'static str],
    /// Out-of-repo metaclass roots: a class deriving from one is a metaclass,
    /// whose `self`/`cls` is a class rather than an instance of its cone.
    pub metaclass_roots: &'static [&'static str],
}

/// The rules for `lang`, or `None` when the language has no receiver lattice.
pub(crate) fn rules(lang: &str) -> Option<&'static LangRules> {
    match lang {
        "python" => Some(&PYTHON),
        _ => None,
    }
}

#[rustfmt::skip]
static PYTHON: LangRules = LangRules {
    builtin_types: &[
        "object", "type", "int", "float", "complex", "bool", "str", "bytes",
        "bytearray", "memoryview", "list", "tuple", "dict", "set", "frozenset",
        "range", "slice", "property", "staticmethod", "classmethod", "super",
        "enumerate", "zip", "map", "filter", "reversed",
        "BaseException", "BaseExceptionGroup", "Exception", "ExceptionGroup",
        "ArithmeticError", "AssertionError", "AttributeError", "BufferError",
        "EOFError", "FloatingPointError", "GeneratorExit", "ImportError",
        "ModuleNotFoundError", "IndexError", "KeyError", "KeyboardInterrupt",
        "LookupError", "MemoryError", "NameError", "NotImplementedError",
        "OSError", "IOError", "EnvironmentError", "OverflowError",
        "RecursionError", "ReferenceError", "RuntimeError", "StopIteration",
        "StopAsyncIteration", "SyntaxError", "IndentationError", "TabError",
        "SystemError", "SystemExit", "TypeError", "UnboundLocalError",
        "UnicodeError", "UnicodeEncodeError", "UnicodeDecodeError",
        "UnicodeTranslateError", "ValueError", "ZeroDivisionError",
        "BlockingIOError", "ChildProcessError", "ConnectionError",
        "BrokenPipeError", "ConnectionAbortedError", "ConnectionRefusedError",
        "ConnectionResetError", "FileExistsError", "FileNotFoundError",
        "InterruptedError", "IsADirectoryError", "NotADirectoryError",
        "PermissionError", "ProcessLookupError", "TimeoutError", "Warning",
        "UserWarning", "DeprecationWarning", "PendingDeprecationWarning",
        "SyntaxWarning", "RuntimeWarning", "FutureWarning", "ImportWarning",
        "UnicodeWarning", "BytesWarning", "ResourceWarning", "EncodingWarning",
    ],
    implicit_root_methods: &[
        "__init__", "__new__", "__init_subclass__", "__setattr__",
        "__getattribute__", "__delattr__", "__eq__", "__ne__", "__hash__",
        "__repr__", "__str__", "__format__", "__reduce__", "__reduce_ex__",
        "__sizeof__", "__dir__", "__subclasshook__", "__getstate__", "__lt__",
        "__le__", "__gt__", "__ge__", "__class_getitem__",
    ],
    metaclass_roots: &["type", "ABCMeta", "EnumMeta", "EnumType"],
};
