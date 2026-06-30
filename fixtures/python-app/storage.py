"""Storage layer: an effectful base class and a subclass override.

Exercises: inheritance, override, own-effects (file + subprocess), a call chain,
and an intraprocedural dataflow chain.
"""

import json
import os
import subprocess


class Store:
    def save(self, record):
        raise NotImplementedError

    def describe(self):
        return "store"


class FileStore(Store):
    def __init__(self, root):
        self.root = root

    def save(self, record):
        payload = serialize(record)
        path = self._path_for(record)
        return write_file(path, payload)

    def _path_for(self, record):
        name = record["id"]
        full = os.path.join(self.root, name)
        return full + ".json"

    def describe(self):
        return "file-store"


def serialize(record):
    raw = json.dumps(record)
    wrapped = raw + "\n"
    return wrapped


def write_file(path, payload):
    with open(path, "w") as handle:
        handle.write(payload)
    return path


def sync_to_remote(path):
    return subprocess.run(["rsync", path, "remote:/backup"])
