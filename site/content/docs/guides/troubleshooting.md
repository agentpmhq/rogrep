---
layout: docs.njk
title: Troubleshooting
description: Diagnose missing sessions, stale data, and terminal problems.
---
Run `rogrep doctor` first. Confirm provider roots are readable and that the provider has created local sessions. Run `rogrep sync` explicitly to surface parse errors. Derived data is disposable; if instructed by a diagnosed version or corruption issue, move the data directory aside and rebuild. For TUI failures, confirm stdin and stdout are attached to a terminal.
