# How it works

Indexing, answers, and the shape of the app. The short version is in the [README](../README.md).

## Indexing

Add a folder in **Folders** and Loft reads every file it can (text and code, PDF, Word,
PowerPoint, spreadsheets, pictures), splits it into chunks, and keeps the index up to date as
files change. While a folder is indexing it shows the file count and, for a large file, how far
through that file it is ("orders.xlsx · 1,200 of 2,000 chunks").

Very large files are indexed **in part**: at most 2,000 chunks (about 3 MB of text) per file.
Data exports such as spreadsheets of orders can hold tens of thousands of rows, which took a
long time to index and made poor answers. The folder says how many files were cut short. A
folder indexed before this limit keeps those files whole until it is rebuilt (**Rebuild
index** in Folders).

## How answers work

1. The question is embedded and the 6 nearest chunks are fetched from the index.
2. Chunks well below the best match are dropped; the rest (up to ~6 KB) go into the prompt.
3. The chat model answers from those excerpts only, streaming token by token. When the
   excerpts don't hold the answer, or the request isn't about the files at all, the reply is
   "I couldn't find that in your indexed files. I'm only really here to talk about items in
   Documents and Notes." (your own folder names; up to four are named, then "and 2 other
   folders"). No sources are shown with it.
4. Small talk is the exception: "hi", "thanks", "how are you?", "who are you?" get a short
   friendly reply from the chat model, with no search and no sources. Only short messages
   made up entirely of such phrases count; "hi, when does the pilot start?" is a question.
5. The answer leads; its sources are in an "N sources" drawer under it (open for the newest
   answer by default; see the Source panel option). The drawer shows the excerpts, and clicking one opens documents and pictures (PDF, Office, txt,
   md, png, jpg…) in their default app; code and scripts are only revealed in the file
   manager, never run. A PDF opens at the cited page when the default PDF app is a browser
   (Edge, Chrome, Firefox…), Acrobat or SumatraPDF on Windows; otherwise it just opens.

### Scans and pictures (OCR)

Pictures (png, jpg, bmp, gif, tiff, webp) are indexed by the text that can be read off them,
and PDF pages with no text layer (scans) are rendered and read the same way, page by page.
A picture with no readable text is not indexed.

- **Windows only for now.** It uses the OCR engine and PDF renderer built into Windows 10/11,
  so nothing is bundled or downloaded; the language is the Windows display language. On macOS
  and Linux pictures and scanned pages yield no text.
- At most 150 pages of one PDF are read by OCR (about 0.3 s a page).
- OCR reads text, not content: a photo, chart or diagram without words in it is only found
  when picture descriptions are on (below).
- Files that had no readable text before this was added are read again on the next launch.

Similarity alone cannot tell an unrelated question from a real one on a small folder (the
scores overlap), so the similarity floor only filters the clearly unrelated and the model does
the rest. Small models are not asked for inline citation numbers because they get them wrong.

### Chat & Answers options

The sliders button beside Send opens a sheet of options. They are remembered between launches
(in the webview's local storage) and never leave the computer; the button turns accent-coloured
when anything differs from the defaults.

| Option | What it does |
|---|---|
| **Answer length** | Concise (one short sentence, capped at 150 tokens), Balanced (one to three sentences) or Detailed (everything relevant). Swaps one rule of the prompt. |
| **Response tone** | Natural, Professional (structured and direct) or Technical (exact names and values, code in backticks). Adds one rule. |
| **Show citations** | Off hides the sources under answers. |
| **Source panel** | Auto: the newest answer's sources are open, earlier ones folded. Always expanded. Collapsed by default. A drawer you open or close by hand stays that way. |
| **Answer from my files only** | On (default): a question the files don't cover gets "I couldn't find that in your indexed files. I'm only really here to talk about items in …" with your folder names. Off: the model may answer from general knowledge, starting with "Your indexed files don't cover this." and listing no sources. |
| **Stream responses** | Off holds the answer back until it is complete. |
| **Remember conversation context** | Off sends each question on its own, so follow-ups are not understood. |
| **Chat history** | Save locally (the default: every conversation is kept in the [chat history](#chat-history)), Current session only (a new conversation lasts until Loft quits), or Don't save (forgotten when the panel closes). |
| **Temperature** | 0 to 1, default 0.2. Higher is more varied and less reliable with a small model. |
| **Maximum output tokens** | 256, 512, 1,024 (default) or 2,048: a hard ceiling on an answer. |

Reset puts everything back; Copy settings copies them as text. With the default options the
prompt is exactly the one the answer tests were tuned on.

### Picture descriptions (optional)

Settings → **Picture descriptions** turns on a small vision model (SmolVLM 500M, a 607 MB
download in two files). While a folder is indexed, each picture gets a one- or two-sentence
description, stored with any text OCR read off it, so "which picture shows a bar chart?"
finds a chart that has no words on it. Off by default.

- The model runs only while pictures are being described (about a second each on a GPU) and
  is stopped when the indexing run ends, so it uses no memory otherwise.
- Descriptions are approximate. A model this small gets the gist ("a lake with snow-capped
  mountains", "a bar chart with four coloured bars") and invents details if allowed to go on,
  so its reply is cut back to its first two sentences and stored as "Automatic description
  (may be imprecise)". Do not rely on it for what a chart's numbers are.
- Turning it on, off, or changing the model reads every indexed picture again.
- Pictures inside PDFs and Office files are not described.

### Context usage

Under each answer Loft shows how many tokens the answer took and how fast it was written
("58 tokens · 61/s"). Below the question box a meter shows how full the model's context window
was for the latest answer ("Context 2% · 640 of 32,768 tokens · 2 turns remembered"); hovering
it breaks the prompt down into excerpts, earlier turns and the question, and says when older
turns were left out. The numbers are the chat server's own counts, and the window size is what
the server reports for a request, not the model's theoretical maximum. A question that is
refused without calling the model, or stopped, has no numbers.

### Follow-up questions

The Ask tab is a conversation: a question is answered with the turns before it, so "Where?"
after "When does the pilot start?" works. **New chat** starts over.

- The turns on screen are sent with each question as context. Saving them is a separate
  matter: see [Chat history](#chat-history).
- Before searching, the chat model rewrites a follow-up into a question that stands on its own
  ("Where does the pilot start?"), because searching for "Where?" finds nothing useful. If the
  rewrite is unusable, the previous question is searched for together with the new one. This
  costs one short extra model call per follow-up; a first question skips it.
- The prompt then holds the last 4 turns at most (about 2,400 bytes; long answers are cut to
  700 characters, oldest turns dropped first), the fresh excerpts, and the question.
- Limits: the model can answer a follow-up from what was already said rather than from the new
  excerpts, in which case the sources listed under it may not be where the answer came from.
  Turns older than the last 4 are forgotten.

### Chat history

Conversations are kept on this computer, in `chats.db` in the app-data folder, and nowhere
else. The bar above the conversation has the history button, the conversation's name (which
opens the history too) and **New chat**.

- **Saved as you go.** The question is stored the moment it is asked, and the conversation is
  named after it (the question, tidied and cut to about 48 characters; no model is asked, so
  the first answer is not delayed). The answer is stored when it ends, with the model that
  wrote it, its usage figures and the excerpts it was written from.
- **Only a finished answer counts as one.** An answer you stopped is kept as *stopped*, one
  that failed as *failed* with the reason, and one that was being written when Loft closed
  becomes *interrupted* at the next launch. Each is shown that way when the conversation is
  reopened.
- **The history panel** lays over the conversation; the window does not change size.
  Conversations are grouped by date, newest first, with the open one marked. Search covers
  titles and everything that was said. The ⋯ menu renames (a name you gave is never replaced
  by an automatic one) and deletes (after asking). Up and Down move through the list from the
  search box, Enter opens, Esc backs out.
- **Resuming** shows a conversation exactly as it was saved (nothing is asked of the model
  again), scrolled to its latest message, ready for a follow-up. Loft reopens the last
  conversation at launch.
- **History is not context.** A conversation may hold a hundred turns; the next question is
  still sent with the last 4 at most, within the same budget as before. The context meter
  shows what was actually sent.
- **Sources stay with their answers.** Each citation is saved as it was shown (file, path,
  page, excerpt, score, order) with a hash of the file's contents. Reopened later, a file
  that is no longer indexed is marked *no longer available* and cannot be opened; one whose
  contents have changed is marked *changed since*. A citation is only ever checked against
  its own path, never matched to another file.
- **Switching is safe.** While an answer is being written you cannot open another
  conversation (stop it first; what was written is kept, as stopped). Every piece of a
  streamed answer carries the id of its question, so text cannot land in another answer.
- **Deleting** a conversation removes its messages and their source records. It is a
  different database from the index: your files and their index are never touched, and
  deleting or rebuilding an index never touches a conversation.
- If a turn cannot be saved, the answer is still shown, with a note saying it was not saved.

Not built (the storage has room for them): pinning, archiving, export, clearing all history,
retention limits, encryption, per-conversation models, and summaries of long conversations.

## Architecture (summary)

```
Notch UI (Svelte + Vite)  ──IPC──►  Rust core (Tauri 2)
                                       ├─ runtime: find the engines, start/stop llama-server
                                       ├─ engine:  which engine runs what, safe switching, power
                                       ├─ models:  catalog, download, select
                                       ├─ index:   walk/parse/chunk/embed, FS watch
                                       ├─ store:   sqlite-vec (vectors + metadata)
                                       └─ rag:     retrieve → prompt → stream
                                     │                         │
                              llama-server (chat)      llama-server (embed)
```

Two local `llama-server` processes: one for chat, one dedicated to embeddings (embedding models
are pooling-based and don't reliably share a process with chat).

[ARCHITECTURE.md](../ARCHITECTURE.md) has the full picture: the code map, the data, the main flows
and the reasons behind the design.
