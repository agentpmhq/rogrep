---
layout: docs.njk
title: Privacy and local-only operation
description: Understand what rogrep reads, writes, and never sends.
---
rogrep has no telemetry, analytics, daemon, account, or normal network path. It reads agent-owned session files and writes rebuildable index, checkpoint, and statistics data only under its own data directory. Delete that derived directory to force a clean rebuild. The website is static and its search index runs in your browser.
