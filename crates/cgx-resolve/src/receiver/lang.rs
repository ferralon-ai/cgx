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
    /// Whether classes have declared bases (`ClassBases` facts). A language
    /// without them (Go) interns its types with no bases at all.
    pub class_lattice: bool,
    /// Out-of-repo type names (last segment) that do not bound a value's
    /// class nominally: protocols, ABCs with builtin registrations or
    /// structural checks, class objects, and typing wrappers. A receiver
    /// declared with one of them is untyped.
    pub untyped_names: &'static [&'static str],
    /// Typing aliases of builtin classes (`List` → `list`).
    pub aliases: &'static [(&'static str, &'static str)],
}

impl LangRules {
    /// The canonical name of an out-of-repo class as written, or `None` when
    /// the name does not bound the value's class.
    pub(crate) fn ext_name<'n>(&self, name: &'n str) -> Option<&'n str> {
        if self.untyped_names.contains(&name) {
            return None;
        }
        Some(
            self.aliases
                .iter()
                .find(|(a, _)| *a == name)
                .map_or(name, |(_, c)| c),
        )
    }
}

/// The rules for `lang`, or `None` when the language is not narrowed.
pub(crate) fn rules(lang: &str) -> Option<&'static LangRules> {
    match lang {
        "python" => Some(&PYTHON),
        "go" => Some(&GO),
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
    class_lattice: true,
    untyped_names: &[
        "object", "type", "Type", "Any", "Callable", "Protocol", "Annotated",
        "ClassVar", "Final", "Literal", "Required", "NotRequired", "ReadOnly",
        "Self", "TypeGuard", "TypeIs", "Never", "NoReturn", "Iterable",
        "Iterator", "Reversible", "Generator", "Container", "Collection",
        "Sized", "Hashable", "Awaitable", "Coroutine", "AsyncIterable",
        "AsyncIterator", "AsyncGenerator", "Mapping", "MutableMapping",
        "Sequence", "MutableSequence", "Set", "AbstractSet", "MutableSet",
        "MappingView", "KeysView", "ItemsView", "ValuesView", "ByteString",
        "Buffer", "SupportsInt", "SupportsFloat", "SupportsComplex",
        "SupportsBytes", "SupportsIndex", "SupportsAbs", "SupportsRound",
        "ContextManager", "AsyncContextManager", "AbstractContextManager",
        "AbstractAsyncContextManager", "IO", "TextIO", "BinaryIO", "PathLike",
        "Number", "Complex", "Real", "Rational", "Integral",
    ],
    aliases: &[
        ("List", "list"), ("Dict", "dict"), ("FrozenSet", "frozenset"),
        ("Tuple", "tuple"), ("DefaultDict", "defaultdict"), ("Deque", "deque"),
    ],
};

#[rustfmt::skip]
static GO: LangRules = LangRules {
    builtin_types: &[
        "error", "string", "bool", "byte", "rune", "int", "int8", "int16",
        "int32", "int64", "uint", "uint8", "uint16", "uint32", "uint64",
        "uintptr", "float32", "float64", "complex64", "complex128", "any",
        "comparable",
    ],
    implicit_root_methods: &[],
    metaclass_roots: &[],
    class_lattice: false,
    untyped_names: &[],
    aliases: &[],
};
