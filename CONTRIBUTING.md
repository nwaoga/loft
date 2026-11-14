# Contributing

Thanks for helping with Loft. Issues and pull requests are welcome.

## Setup

Prerequisites and how to run the app are in [README.md](README.md). In short:

```bash
npm install
npm run fetch-llama    # once, for a desktop build
npm run dev            # UI only, mock data
npm run tauri dev      # macOS / Linux
```

On Windows, use `scripts\win-dev.bat` so the MSVC toolset is selected.

Try the app on [sample-docs/](sample-docs/README.md). Do not add files from a personal or work folder, and do not commit databases (`*.db`, `*.sqlite`), model files, llama.cpp binaries, or `.env` files.

## Checks

```bash
npm run check
cd src-tauri && cargo test --lib
```

The ignored Rust tests need downloaded models. Say what you ran, including when you could not run a platform's build.

## Pull requests

- Describe what changed and how you tested it.
- Keep the change focused. Version bumps belong with a release, not a drive-by edit.
- Loft is offline and local. A change should not upload folders, documents, or chat history.
