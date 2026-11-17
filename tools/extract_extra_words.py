"""Выгружает static let ...: Set<String> из ExtraWords.swift в JSON (имя набора -> список слов)."""
import json, re, sys

src = open(sys.argv[1], encoding="utf-8").read()
out = {}
for m in re.finditer(r"static let (\w+): Set<String> = \[(.*?)\n?\s*\]", src, re.S):
    name, body = m.group(1), m.group(2)
    body = re.sub(r"//[^\n]*", "", body)          # комментарии
    words = re.findall(r'"((?:[^"\\]|\\.)*)"', body)
    words = [re.sub(r"\\u\{([0-9a-fA-F]+)\}", lambda x: chr(int(x.group(1), 16)), w).replace('\\"', '"').replace("\\\\", "\\") for w in words]
    out[name] = sorted(set(words))
json.dump(out, open(sys.argv[2], "w", encoding="utf-8"), ensure_ascii=False, indent=0)
for k, v in out.items():
    print(k, len(v))
