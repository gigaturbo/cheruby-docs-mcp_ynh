"""Scores search_documents on a running server against a set of queries.

    python3 tests/eval_search.py QUERIES TOKEN_FILE [--url URL] [--top N]

QUERIES is a JSON list kept outside the repository, since it names the
user's own files:

    [{"query": "how to wash wool by hand", "expect": ["Notes/Lessive.md"]}, ...]

A hit counts when its path equals an expected entry or lies under it, so
"Documents/Emploi/Old/Pole_Emploi" accepts any file in that folder. Each
query's rank is that of its first counted hit, among the files returned
(several passages of one file count once). TOKEN_FILE holds the bearer
token alone; it is never taken as an argument, which would show it to
other processes and to sudo's log.

Prints each query's rank and time, then the count in the top 5 and the
mean reciprocal rank. Exits 1 when a query is outside the top 5.
"""

import argparse
import json
import re
import sys
import time
import urllib.request

HIT = re.compile(r"^--- (.+?)(?:, page \d+)? \(\d{4}-\d\d-\d\d\) ---$", re.M)


def search(url, token, query, top):
    """(files in result order, seconds) for one query."""
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                       "params": {"name": "search_documents", "arguments": {"query": query, "limit": top}}})
    req = urllib.request.Request(url, body.encode(), {
        "Content-Type": "application/json", "Accept": "application/json, text/event-stream",
        "Authorization": f"Bearer {token}"})
    start = time.monotonic()
    with urllib.request.urlopen(req, timeout=120) as resp:
        result = json.load(resp)["result"]
    took = time.monotonic() - start
    text = result["content"][0]["text"]
    if result.get("isError"):
        sys.exit(f"{query!r}: {text}")
    files = []
    for path in HIT.findall(text):
        if path not in files:
            files.append(path)
    return files, took


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("queries")
    parser.add_argument("token_file")
    parser.add_argument("--url", default="http://127.0.0.1:8770/mcp")
    parser.add_argument("--top", type=int, default=10, help="passages asked for, 1 to 10")
    args = parser.parse_args()
    with open(args.token_file, encoding="utf-8") as f:
        token = f.read().strip()
    with open(args.queries, encoding="utf-8") as f:
        cases = json.load(f)

    ranks, times = [], []
    for case in cases:
        files, took = search(args.url, token, case["query"], args.top)
        rank = next((n for n, path in enumerate(files, 1)
                     if any(path == e or path.startswith(e.rstrip("/") + "/") for e in case["expect"])), None)
        ranks.append(rank)
        times.append(took)
        print(f"{rank or '-':>3}  {took:5.1f} s  {case['query']}")
        if not rank or rank > 5:
            print(f"     got: {', '.join(files[:5]) or 'nothing'}")
    top5 = sum(1 for r in ranks if r and r <= 5)
    mrr = sum(1 / r for r in ranks if r) / len(ranks)
    print(f"\n{top5} of {len(ranks)} in the top 5, mean reciprocal rank {mrr:.2f}, "
          f"slowest {max(times):.1f} s, mean {sum(times) / len(times):.1f} s")
    sys.exit(0 if top5 == len(ranks) else 1)


if __name__ == "__main__":
    main()
