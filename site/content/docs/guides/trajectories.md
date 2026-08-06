---
layout: docs.njk
title: Git and PR trajectories
description: Trace captured branches, commits, and pull requests to sessions.
---
`rogrep git CONVERSATION_ID` prints captured Git and GitHub operations. `rogrep trajectory --pr N`, `--branch NAME`, or `--commit SHA` ranks conversations that led to an artifact. Inspect the emitted show commands. Treat text-only mentions as weaker evidence than captured operations.
