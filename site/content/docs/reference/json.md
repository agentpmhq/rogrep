---
layout: docs.njk
title: JSON output conventions
description: Use rogrep's JSON output safely in automation.
---
Use `--json` when consuming output programmatically and default text when exploring. JSON preserves IDs and exact structured records needed for follow-up commands. Do not scrape styled text. Missing metadata, empty timelines, and incomplete sources do not prove an event never occurred; preserve those semantics in automation.
