## Bug: BOLT_ALLOW_SELF_SIGNED documented but not implemented in main branch

**Layer**: runtime-topology × api-contracts
**Severity**: Medium
**Pass**: 1
**Evidence**:

- `docs/index.md:107` documents `BOLT_ALLOW_SELF_SIGNED`:
- code_quote: `| \`BOLT_ALLOW_SELF_SIGNED\` | (none) | Set to \`1\` (literal only) to permit self-signed certs |`
- `grep -rn 'BOLT_ALLOW_SELF_SIGNED' --include='*.rs'` returns zero matches in main source tree
- Implementation exists only in a worktree branch:
  `worktrees/feat/issue--execute-github-issue-736-ws-a-deploy-secrets/sedandelivery-bolt/src/tls.rs`

**Impact**: Operators setting `BOLT_ALLOW_SELF_SIGNED=1` for dev/staging TLS get no effect.
Feature was documented before it was merged to main.

**Fix**: Either merge the TLS self-signed check from the worktree branch, or remove from docs
until it ships.
