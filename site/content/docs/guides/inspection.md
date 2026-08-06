---
layout: docs.njk
title: Exchanges and inspection
description: Drill from search results into exact conversation evidence.
---
Start broad, then use `rogrep find CONVERSATION_ID QUERY` to locate turns inside one conversation. `rogrep show CONVERSATION_ID` renders it; pass an exchange reference (`rg_&#eN`) or `--around TURN` to narrow the window. Conversation-scoped find includes turns hidden from default corpus search.
