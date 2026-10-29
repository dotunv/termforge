# ADR 0019: Task planning metadata is durable and relational

- Status: Accepted
- Date: 2026-10-28
- Extends: ADR 0016 (project workbench)

## Context

Task priority and dependency controls are not useful if they exist only in a
view model. They must survive daemon and application restarts, remain available
to the CLI, and reject impossible graphs. Dependency identifiers also cannot be
trusted merely because a client sent them: tasks are project-scoped and older
or concurrent clients may submit stale data.

## Decision

- Append a store migration adding a constrained priority column and a
  `task_dependencies` relation with cascading foreign keys.
- Represent priority and dependency identifiers in `tf-proto`; this is a
  protocol change and increments the protocol version.
- Validate dependency writes in `tf-store`: every dependency must exist in the
  same project, duplicates and self-dependencies are rejected, and recursive
  reachability checks prevent cycles.
- Replace a task's dependency set transactionally. `None` in an update leaves
  dependencies unchanged, while an empty list clears them.
- Keep filtering and sorting as presentation state. Priority and dependencies
  are durable task data shared by the GUI and CLI.

## Consequences

- Old databases migrate without changing prior migration entries; existing
  tasks receive normal priority and no dependencies.
- Clients and `forged` must run the same protocol version, as they already do
  for every incompatible protocol change.
- Deleting a prerequisite removes dependency edges but does not delete tasks
  that depended on it.
