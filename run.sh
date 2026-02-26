#!/bin/bash
set -e
unset CLAUDECODE
cd /tmp/amplihack-workstreams/ws-15-planner
claude -p "$(cat <<'PROMPT'
You are working on branch feat/tck-planner-executor in a Rust GQL/Cypher query engine.

Read TASK.md for your full assignment. Follow the default-workflow recipe:
1. Read and understand the task requirements in TASK.md
2. Investigate the codebase - read the key files mentioned in TASK.md
3. Plan your implementation approach
4. Implement all changes
5. Run tests: cargo test -p gql-core --lib && cargo test -p gql-core --test end_to_end
6. Run cargo clippy -p gql-core -- -D warnings
7. Run cargo fmt --all
8. Run TCK tests: cargo test --test tck 2>&1 | tail -30
9. Git add, commit with descriptive message, push to origin
10. Create PR against main using gh pr create

CRITICAL: Your goal is 100% TCK pass rate. Be thorough. Read the planner and executor code carefully before making changes. Test incrementally.

Start by reading TASK.md, then the key source files.
PROMPT
)" --max-turns 200 2>&1
