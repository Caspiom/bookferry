#!/usr/bin/env python3
import json
import re

linux_books = {}
with open("linux-cfi-results.jsonl") as f:
    for line in f:
        obj = json.loads(line)
        tag = obj.get("tag", "")
        if tag.startswith("RISCO3_EPUB_"):
            data = obj["data"]
            linux_books[data["epub"]] = data

with open("android-cfi-logcat.txt") as f:
    lines = f.readlines()

chunks = []
acc = ""
for l in lines:
    m = re.search(r"I BookferrySpike: (?:\[part (\d+)/(\d+)\] )?(.*)", l)
    if not m:
        continue
    p, tot, body = m.group(1), m.group(2), m.group(3).rstrip("\r\n")
    if p is None:
        chunks.append(body)
    else:
        acc += body
        if p == tot:
            chunks.append(acc)
            acc = ""

android_books = {}
for c in chunks:
    m = re.match(r"\[BOOKFERRY_RESULT\]\[(RISCO3_EPUB_[^\]]+)\] (.*)", c)
    if m:
        data = json.loads(m.group(2))
        android_books[data["epub"]] = data

with open("android-cfi-results.json", "w") as f:
    json.dump(android_books, f, indent=2)

for book in ["frankenstein-pg84.epub", "moby-dick-pg2701.epub", "pride-and-prejudice-pg1342.epub"]:
    lb = linux_books[book]
    ab = android_books[book]
    print(f"\n==================== {book} ====================")
    print(f"Linux elapsed: {lb['elapsed_ms']} ms | Android elapsed: {ab['elapsed_ms']} ms")
    print("--- 1. DOM Chapters (5 chapters via resolveHref) ---")
    for lc, ac in zip(lb["dom_chapters"], ab["dom_chapters"]):
        match = (lc["cfi"] == ac["cfi"]) and (lc["snippet"] == ac["snippet"])
        print(f"  [{lc['label']}]: match={match}")
        print(f"    Linux CFI  : {lc['cfi']}")
        print(f"    Android CFI: {ac['cfi']}")
        print(f"    Snippet    : {lc['snippet'][:70]}")
    print("--- 2. DOM Fractions (10%, 50%, 90% via SectionProgress + char offset) ---")
    for lf, af in zip(lb["dom_fractions"], ab["dom_fractions"]):
        match = (lf["cfi"] == af["cfi"]) and (lf["snippet"] == af["snippet"])
        print(f"  [{int(lf['fraction']*100)}%]: match={match} (chars={lf['total_chars_in_section']}/{af['total_chars_in_section']})")
        print(f"    Linux CFI  : {lf['cfi']}")
        print(f"    Android CFI: {af['cfi']}")
        print(f"    Snippet    : {lf['snippet'][:70]}")
    print("--- 3. Paginator Chapters (goTo(href) in rendered <foliate-view>) ---")
    for lc, ac in zip(lb["paginator_chapters"], ab["paginator_chapters"]):
        m_start = lc["collapsed_start_cfi"] == ac["collapsed_start_cfi"]
        m_full = lc["full_range_cfi"] == ac["full_range_cfi"]
        print(f"  [{lc['label']}]: start_match={m_start}, full_range_match={m_full}")
        print(f"    Linux start  : {lc['collapsed_start_cfi']} | full: {lc['full_range_cfi']}")
        print(f"    Android start: {ac['collapsed_start_cfi']} | full: {ac['full_range_cfi']}")
    print("--- 4. Paginator Fractions (goToFraction(0.1/0.5/0.9) in rendered <foliate-view>) ---")
    for lf, af in zip(lb["paginator_fractions"], ab["paginator_fractions"]):
        m_start = lf["collapsed_start_cfi"] == af["collapsed_start_cfi"]
        m_full = lf["full_range_cfi"] == af["full_range_cfi"]
        print(f"  [{int(lf['fraction']*100)}%]: start_match={m_start}, full_range_match={m_full}")
        print(f"    Linux start  : {lf['collapsed_start_cfi']} | snippet: {lf['snippet'][:60]}")
        print(f"    Android start: {af['collapsed_start_cfi']} | snippet: {af['snippet'][:60]}")
