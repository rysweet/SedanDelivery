#!/bin/bash
cd "/tmp/amplihack-workstreams/ws-134"
unset CLAUDECODE
amplihack claude --subprocess-safe -- -p "@TASK.md

Execute task autonomously following DEFAULT_WORKFLOW.md.
NO QUESTIONS. Work through all steps. Create PR when complete."
