"""Writes src-tauri/engines.json: the llama.cpp builds Loft can use, per
platform, with the size and SHA-256 GitHub publishes for each archive of the
pinned release. Re-run after changing TAG to pin another release."""
import json
import sys
import urllib.request

TAG = "b12110"
REPO = "ggml-org/llama.cpp"

# platform -> backend -> (optional?, [asset name patterns], note)
BUILDS = {
    "win-x64": {
        "cpu": (False, ["llama-{tag}-bin-win-cpu-x64.zip"], None),
        "vulkan": (False, ["llama-{tag}-bin-win-vulkan-x64.zip"], None),
        "cuda": (True, ["llama-{tag}-bin-win-cuda-13.4-x64.zip", "cudart-llama-bin-win-cuda-13.4-x64.zip"],
                 "CUDA 13.4 for NVIDIA graphics cards; needs a current NVIDIA driver."),
        "openvino": (True, ["llama-{tag}-bin-win-openvino-2026.4.1-x64.zip"],
                     "OpenVINO 2026.4.1 for Intel processors, graphics and NPUs. Experimental."),
    },
    "win-arm64": {
        "cpu": (False, ["llama-{tag}-bin-win-cpu-arm64.zip"], None),
        "vulkan": (False, ["llama-{tag}-bin-win-vulkan-arm64.zip"], None),
        "cuda": (True, ["llama-{tag}-bin-win-cuda-13.4-arm64.zip", "cudart-llama-bin-win-cuda-13.4-arm64.zip"],
                 "CUDA 13.4 for NVIDIA graphics cards; needs a current NVIDIA driver."),
    },
    "mac-arm64": {"metal": (False, ["llama-{tag}-bin-macos-arm64.tar.gz"], None)},
    "mac-x64": {"cpu": (False, ["llama-{tag}-bin-macos-x64.tar.gz"], None)},
    "linux-x64": {
        "cpu": (False, ["llama-{tag}-bin-ubuntu-x64.tar.gz"], None),
        "vulkan": (False, ["llama-{tag}-bin-ubuntu-vulkan-x64.tar.gz"], None),
        "cuda": (True, ["llama-{tag}-bin-ubuntu-cuda-13.4-x64.tar.gz", "cudart-llama-{tag}-bin-ubuntu-cuda-13.4-x64.tar.gz"],
                 "CUDA 13.4 for NVIDIA graphics cards; needs a current NVIDIA driver."),
        "openvino": (True, ["llama-{tag}-bin-ubuntu-openvino-2026.4.1-x64.tar.gz"],
                     "OpenVINO 2026.4.1 for Intel processors, graphics and NPUs. Experimental."),
    },
    "linux-arm64": {
        "cpu": (False, ["llama-{tag}-bin-ubuntu-arm64.tar.gz"], None),
        "vulkan": (False, ["llama-{tag}-bin-ubuntu-vulkan-arm64.tar.gz"], None),
    },
}

request = urllib.request.Request(
    f"https://api.github.com/repos/{REPO}/releases/tags/{TAG}",
    headers={"User-Agent": "loft-engines-manifest", "Accept": "application/vnd.github+json"},
)
release = json.load(urllib.request.urlopen(request, timeout=60))
assets = {a["name"]: a for a in release["assets"]}

manifest = {
    "comment": "The llama.cpp builds Loft can use, from one pinned release. `sha256` and `size` are the values GitHub "
               "publishes for each archive; a download that does not match is discarded. Written by "
               "scripts/make-engines-manifest.py (see scripts/fetch-llama.mjs); do not edit by hand.",
    "repo": REPO,
    "tag": TAG,
    "builds": {},
}
for platform, backends in BUILDS.items():
    manifest["builds"][platform] = {}
    for backend, (optional, patterns, note) in backends.items():
        archives = []
        for pattern in patterns:
            name = pattern.format(tag=TAG)
            asset = assets.get(name)
            if asset is None:
                sys.exit(f"release {TAG} has no asset named {name}")
            digest = (asset.get("digest") or "")
            if not digest.startswith("sha256:"):
                sys.exit(f"GitHub publishes no SHA-256 for {name}")
            archives.append({"name": name, "size": asset["size"], "sha256": digest[len("sha256:"):],
                             "url": asset["browser_download_url"]})
        entry = {"optional": optional, "archives": archives}
        if note:
            entry["note"] = note
        manifest["builds"][platform][backend] = entry

out = sys.argv[1]
with open(out, "w", encoding="utf-8", newline="\n") as f:
    json.dump(manifest, f, indent=2)
    f.write("\n")
total = sum(len(b) for b in manifest["builds"].values())
print(f"wrote {out}: {total} builds for {len(manifest['builds'])} platforms, release {TAG}")
for backend, entry in manifest["builds"]["win-x64"].items():
    size = sum(a["size"] for a in entry["archives"]) / 1e6
    print(f"  win-x64 {backend:9} {'optional' if entry['optional'] else 'default '} {size:7.1f} MB  {len(entry['archives'])} archive(s)")
