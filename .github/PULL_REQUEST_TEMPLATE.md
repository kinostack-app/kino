<!--
Thanks for sending a PR! A few things to check before submitting:
- Run `cd backend && just fix-ci && just test`
- For frontend changes: `cd frontend && npm run lint && npm run typecheck && npm run test`
- For backend schema changes: `cd backend && just codegen` to regenerate frontend types

The template below helps reviewers; delete sections that don't apply.
-->

## Summary

<!-- One or two sentences. What changes, and why. -->

## Subsystem(s) touched

<!-- e.g. subsystem 04 import, subsystem 22 tray. Link to docs/subsystems/<n>-<name>.md if applicable. -->

## Type

- [ ] Bug fix
- [ ] New feature
- [ ] Refactor (no behaviour change)
- [ ] Docs / specs only
- [ ] Build / CI / release machinery
- [ ] Schema change (migration + codegen)

## Testing

<!-- How did you verify this? Unit tests, manual, both? -->

## Screenshots / recordings

<!-- For UI changes. Optional otherwise. -->

## Notes for reviewer

<!-- Specific files to look at, trade-offs you weighed, things you're unsure about. -->

## DCO sign-off

This project uses the [Developer Certificate of Origin](https://developercertificate.org/).
Every commit on this PR must carry a `Signed-off-by:` trailer (use
`git commit -s`). See [`CONTRIBUTING.md`](../CONTRIBUTING.md) for details.

- [ ] All commits in this PR are signed off (`git commit -s`)
