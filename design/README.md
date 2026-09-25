# LLM provider settings redesign

- Editable pen.dev canvas: `llm-redesign.pen` (final screens are the first two frames; earlier explorations remain below them).
- Browser review: `exports/preview.html` (Overview / Model dialog).
- High-resolution exports: `exports/final-overview.png`, `exports/final-dialog.png`.

The overview keeps one card per provider. Model rows show only names and assigned roles; selecting a row opens the detail dialog. Provider actions live in the card menu, and adding a provider is a single header action.

The dialog presents the model ID, role assignment, read-only image-input status, and removal. The current application has no per-model image-input override: only OpenAI-compatible and Azure AI Foundry connections expose an image-input override, which applies to every model on that connection. Role assignment also remains global across providers.
