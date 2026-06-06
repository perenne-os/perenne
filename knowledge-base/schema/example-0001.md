---
id: KB-0001
title: "Git SSH-signed commits fail with passphrase prompt on Windows"
status: fixed
severity: medium
component: dev-environment
symptoms:
  - "git commit fails non-interactively with: 'Enter passphrase: Load key ... incorrect passphrase supplied to decrypt private key'"
  - "fatal: failed to write commit object"
  - "Signing works when invoked via the system ssh-keygen directly, but git's own commit signing fails"
diagnosis: >
  Commit signing is configured with gpg.format=ssh and a passphrase-protected
  SSH key. Git for Windows ships its own bundled ssh-keygen which cannot reach
  the Windows OpenSSH ssh-agent (a named pipe), so it tries to load the
  encrypted private key file directly and prompts for a passphrase. In a
  non-interactive context the prompt cannot be answered and the commit fails.
playbook:
  - "Start the Windows ssh-agent service and set it to start automatically: 'Get-Service ssh-agent | Set-Service -StartupType Automatic; Start-Service ssh-agent' (Administrator)."
  - "Cache the key once: 'ssh-add $env:USERPROFILE\\.ssh\\id_ed25519' (enter passphrase once). Verify with 'ssh-add -l'."
  - "Point git at the system OpenSSH ssh-keygen (which can reach the agent): 'git config --global gpg.ssh.program \"C:/Windows/System32/OpenSSH/ssh-keygen.exe\"'."
  - "Reverse if needed: 'git config --global --unset gpg.ssh.program' and/or 'ssh-add -d' to remove the cached key."
verification: "Run a 'git commit' non-interactively; it completes and the commit is signed (no passphrase prompt). Confirm with 'git log --show-signature -1'."
created: 2026-06-06
updated: 2026-06-06
references:
  - "docs/learning/0001-dev-environment.md"
  - "knowledge-base/schema/issue-record.md"
---

## Notes

This is the project's first real knowledge-base entry — recorded during Phase 0
setup, and a good illustration of the intended workflow: a concrete problem was
hit, diagnosed to a root cause, and resolved with a reversible playbook that
another developer (or, later, the OS itself) can apply on recurrence.

The deeper signal: signing worked fine through the *system* OpenSSH but not
through git's bundled copy. The fix was not "disable signing" (which would have
weakened security) but redirecting git to the toolchain that integrates with the
agent — preserving the security property while removing the friction.
