---
layout: docs.njk
title: First sync and core concepts
description: Index sessions and understand rogrep's data model.
---
~~~sh
rogrep sync
rogrep doctor
rogrep stats projects
~~~

The first command discovers and indexes on-disk history. Later commands refresh incrementally.

## The data model

- **Conversation** (`rg&`): one provider session.
- **Exchange** (`rg&#eN`): one real user prompt and everything the agent did in response.
- **Turn**: one message, tool call, or tool result.

Use IDs from results with `rogrep show`.
