# Architecture Docs

Start with [Full-Scale Project Shape](full-scale-project-shape.md) for the Loom/Devbox split.
[Loom Storage Consistency](loom-storage-consistency.md) records the current testable guarantees and
non-guarantees for cursors, compare-and-set, sparse hydration, eviction, pins, and hosted
metadata/object integrity.
[Native Filesystem Adapter Alpha](native-filesystem-adapter.md) records the OS adapter boundary,
support matrix, and fail-closed native stubs before real host integrations.

Most other pages in this directory are legacy alpha architecture records. They are kept because the
old alpha code still compiles and remains useful for smoke tests, but their `project` and `snapshot`
language is compatibility-era terminology. New architecture work should use shared folder, object,
file version, folder revision, checkpoint, pin, cursor, sandbox, and overlay.
