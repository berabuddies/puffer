---
name: project-memory
description: Load durable project memory for the active configured project.
allowed-tools:
  - Read
  - Memory
argument-hint: <memory-file>
user-invocable: true
disable-model-invocation: false
---

Load the active project's durable memory from `$ARGUMENTS`.

1. Use `Read` on `$ARGUMENTS`.
2. Treat the file contents as durable project-specific context for the current task.
3. If the file is missing or empty, continue without inventing memory.
4. Do not quote the full file back to the user unless it is necessary.
5. If you learn a durable new fact, use the `Memory` tool to add, replace, or remove entries. Do not edit `MEMORY.md` directly.
