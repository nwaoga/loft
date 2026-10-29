"""Download pinned CPU prototype assets from their publishers; verify hashes."""
import concurrent.futures
import hashlib
import json
from pathlib import Path
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parent
MODELS = [
    ("Qwen/Qwen2.5-1.5B-Instruct-GGUF", "91cad51170dc346986eccefdc2dd33a9da36ead9", "qwen2.5-1.5b-instruct-q4_k_m.gguf"),
    ("nomic-ai/nomic-embed-text-v1.5-GGUF", "0188c9bf409793f810680a5a431e7b899c46104c", "nomic-embed-text-v1.5.Q4_K_M.gguf"),
]
RUNTIME_URL = "https://github.com/ggml-org/llama.cpp/releases/download/b11969/llama-b11969-bin-win-cpu-x64.zip"
RUNTIME_SHA = "f3cc2c9ab938ad76b6a4bc9eeb8f81cb25455ab291736ee767360758112fa672"


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def download(url, path, expected, cancel=None, progress=None):
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists() and digest(path) == expected:
        return
    partial = path.with_suffix(path.suffix + ".part")
    print(f"Downloading {path.name}", flush=True)
    with urllib.request.urlopen(url, timeout=120) as response, partial.open("wb") as output:
        while block := response.read(1024*1024):
            if cancel is not None and cancel.is_set():
                raise InterruptedError("Download cancelled")
            output.write(block)
            if progress:
                progress(output.tell())
    if digest(partial) != expected:
        raise ValueError(f"Checksum mismatch: {path}")
    partial.replace(path)


def model(info, cancel=None, progress=None):
    repo, revision, filename = info
    with urllib.request.urlopen(f"https://huggingface.co/api/models/{repo}/revision/{revision}?blobs=true", timeout=60) as response:
        metadata = json.load(response)
    record = next(f for f in metadata["siblings"] if f["rfilename"] == filename)
    expected = record["lfs"]["sha256"]
    url = f"https://huggingface.co/{repo}/resolve/{revision}/{filename}"
    download(url, ROOT / "models" / filename, expected, cancel, progress)
    return {"repo": repo, "revision": revision, "file": filename, "sha256": expected,
            "bytes": record["lfs"]["size"], "license": "Apache-2.0", "url": url}


def main():
    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
        futures = [pool.submit(model, info) for info in MODELS]
        runtime = pool.submit(download, RUNTIME_URL, ROOT / "runtime" / "llama.zip", RUNTIME_SHA)
        records = [future.result() for future in futures]
        runtime.result()
    destination = (ROOT / "runtime" / "b11969").resolve()
    destination.mkdir(exist_ok=True)
    with zipfile.ZipFile(ROOT / "runtime" / "llama.zip") as archive:
        for member in archive.infolist():
            target = (destination / member.filename).resolve()
            if not target.is_relative_to(destination):
                raise ValueError("Unsafe archive member")
        archive.extractall(destination)
    server = next(destination.rglob("llama-server.exe"))
    manifest = {"runtime": {"url": RUNTIME_URL, "archive_sha256": RUNTIME_SHA,
                           "server": str(server.relative_to(ROOT)), "server_sha256": digest(server)},
                "models": records}
    (ROOT / "models" / "manifest.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    print("Verified runtime and models installed locally.", flush=True)


if __name__ == "__main__":
    main()
