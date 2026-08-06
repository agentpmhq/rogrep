---
layout: docs.njk
title: Searching
description: Search conversations and exchanges with text and facets.
---
Use `rogrep QUERY` for corpus search and `rogrep x QUERY` when the exchangethe prompt and all following actionsis the useful unit. Terms combine with AND. Phrases, regex, dates, project filters, tool results, files, and Git activity are covered in [query syntax](/docs/query-syntax/).

~~~sh
rogrep "incremental parser" provider:codex since:30d
rogrep x tool_status:failed tool_cmd:cargo
~~~

Use `--sort recent` for chronology and `--json` for programs.
