# Loft handover

## Current state

Loft is a working Windows Tkinter prototype, not a packaged release. The desktop UI supports selected folders, manual indexing and refresh, file-grounded search across Everything/Documents/Code, a separate Chat mode, local answer-model selection, and Windows appearance controls. The selected answer model runs locally through llama.cpp; Nomic embeddings stay separate.

The repository does not include downloaded GGUF files, llama.cpp binaries, the virtual environment, user indexes, logs, or benchmark output. Follow `README.md` to set up a fresh machine. The local development workspace currently has 12 indexed source files across two roots; those local data are intentionally not part of the repository.

## Recent decisions

- Model choices are shown directly in Settings with Download, Add local file, Use, and In use actions. Adding an existing GGUF computes a local hash without a network request.
- Folders appear in the list immediately after selection. The folder window shows indexing stages and final updated/unchanged/removed counts. A failed index can be retried.
- Model startup percentages represent observed startup milestones. The local server must be reachable on loopback; the desktop preview was verified outside a sandbox that blocked loopback sockets.
- Greetings use the selected answer model. Chat mode handles general conversation without file evidence. Search mode with no supporting passage does not invent a file answer.
- Source IDs are constrained and validated, but answer claims are not fully verified against cited text.

## Known limitations and next work

1. Improve retrieval quality and evaluate it on real user questions. The lexical candidate gate misses synonym-only queries, while broad matches can retrieve weak passages. Measure answer relevance and citation support before loosening that gate.
2. Add automatic folder watching and clear per-folder index state. A root is registered before indexing, so it can be listed with no indexed files after failure or cancellation.
3. Make long answers and error states easier to inspect in the desktop UI. Test navigation and model operations on multiple screen sizes.
4. Measure memory, latency, and battery use on modest machines. The 1.5B answer model is provisional; the 0.5B candidate had unreliable citation selection in earlier checks.
5. Add document readers, packaging, crash recovery, and a scalable vector index only after the core retrieval and answer quality is validated.

## Verification and operating notes

Run `.venv/Scripts/python -m unittest`, `.venv/Scripts/python loft_preview.py --check`, and, when model files are installed, `.venv/Scripts/python verify_engine.py`. `verify_engine.py` writes an ignored report under `results/`. Start the app with `.venv/Scripts/python loft_preview.py --expanded`. If a model appears stuck loading, check local loopback access and `data/logs/` before changing model settings.

No Azure DevOps project or planning files are configured in this workspace; this document tracks the local handover.
