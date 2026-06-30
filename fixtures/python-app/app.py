"""Application entrypoint: imports the storage layer, builds a record through a
dataflow chain, and persists it.

Exercises: cross-module import, function-calls-function chain, entrypoint hint
(`if __name__ == "__main__"`), and a dataflow chain (a -> b -> c -> return).
"""

from storage import FileStore, sync_to_remote


def build_record(seed):
    base = seed
    scaled = base * 2
    record = {"id": "rec", "value": scaled}
    return record


def run(seed):
    record = build_record(seed)
    store = FileStore("/tmp/data")
    path = store.save(record)
    sync_to_remote(path)
    return path


def main():
    run(7)


if __name__ == "__main__":
    main()
