"""Headless entry point for Loft's real local engine."""
import argparse
import json
import threading
from loft_engine import Engine


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--database")
    commands = parser.add_subparsers(dest="command", required=True)
    index = commands.add_parser("index")
    index.add_argument("folder")
    ask = commands.add_parser("ask")
    ask.add_argument("question")
    ask.add_argument("--scope", choices=["All", "Documents", "Code"], default="All")
    commands.add_parser("folders")
    args = parser.parse_args()
    engine = Engine(args.database)
    cancel = threading.Event()
    def emit(kind, value):
        if kind == "token":
            print(value, end="", flush=True)
        elif kind != "sources":
            print(f"\n{kind}: {value}", flush=True)
        else:
            print(json.dumps(value, indent=2), flush=True)
    try:
        if args.command == "index":
            print(json.dumps(engine.index_folder(args.folder, cancel, emit), indent=2))
        elif args.command == "ask":
            result = engine.ask(args.question, args.scope, cancel, emit)
            print(f"\nCompleted in {result['seconds']:.2f}s")
        else:
            print(json.dumps(engine.roots(), indent=2))
    except KeyboardInterrupt:
        cancel.set()
        raise SystemExit(130)


if __name__ == "__main__":
    main()
