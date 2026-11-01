"""Check relative links and anchors in all tracked Markdown files.

Usage: python -I scripts/ci/linkcheck.py [REPO_ROOT]   (defaults to ".")
- Every relative link target must exist (file or directory).
- Every #anchor into a Markdown file must match a GitHub-style heading slug
  (or an explicit <a id/name> anchor) in that file.
- Links into gitignored target/ are allowed only when the surrounding line
  labels them as local artifacts ("local artifact" / "not in a clone" etc.).
Exit code 1 when anything is broken.
"""
import os, posixpath, re, subprocess, sys, unicodedata
from urllib.parse import unquote

root = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else ".")
os.chdir(root)
files = subprocess.run(["git", "ls-files", "-z", "*.md"], capture_output=True, check=True).stdout.decode("utf-8").split("\0")
files = [f for f in files if f]

FENCE = re.compile(r"^\s{0,3}(`{3,}|~{3,})")
INLINE_CODE = re.compile(r"(`+)(.+?)\1")
INLINE_LINK = re.compile(r"!?\[(?:[^\[\]]|\[[^\[\]]*\])*\]\(\s*<?([^)\s>]+)>?(?:\s+\"[^\"]*\")?\s*\)")
REF_DEF = re.compile(r"^\s{0,3}\[[^\]]+\]:\s*<?(\S+?)>?(?:\s+.*)?$")
HEADING = re.compile(r"^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$")
HTML_ANCHOR = re.compile(r"<a\s+[^>]*(?:id|name)\s*=\s*\"([^\"]+)\"", re.I)
LOCAL_LABEL = re.compile(r"local artifact|not in (?:a|the) (?:fresh )?clone|gitignored|local evidence|local-only", re.I)


def slugify(text):
    # GitHub: strip markdown inline formatting, lowercase, drop punctuation except - and _, spaces -> -
    text = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"<[^>]+>", "", text)
    text = text.replace("`", "").replace("*", "")
    text = re.sub(r"(?<!\w)_|_(?!\w)", "", text)
    text = text.strip().lower()
    out = []
    for ch in text:
        cat = unicodedata.category(ch)
        if ch in "-_" or ch == " ":
            out.append("-" if ch == " " else ch)
        elif cat[0] in "LNM":
            out.append(ch)
    return "".join(out)


anchor_cache = {}


def anchors_for(path):
    if path in anchor_cache:
        return anchor_cache[path]
    seen, result = {}, set()
    in_fence = None
    for line in open(path, encoding="utf-8").read().splitlines():
        m = FENCE.match(line)
        if m:
            marker = m.group(1)[0]
            if in_fence is None:
                in_fence = marker
            elif in_fence == marker:
                in_fence = None
            continue
        if in_fence:
            continue
        for a in HTML_ANCHOR.findall(line):
            result.add(a)
        h = HEADING.match(line)
        if h:
            base = slugify(h.group(2))
            n = seen.get(base, 0)
            seen[base] = n + 1
            result.add(base if n == 0 else f"{base}-{n}")
    anchor_cache[path] = result
    return result


ignored_cache = {}


def is_ignored(path):
    if path not in ignored_cache:
        r = subprocess.run(["git", "check-ignore", "-q", path])
        ignored_cache[path] = r.returncode == 0
    return ignored_cache[path]


broken, total, anchors_checked, local_ok = [], 0, 0, 0
for f in files:
    d = posixpath.dirname(f)
    in_fence = None
    for lineno, line in enumerate(open(f, encoding="utf-8").read().splitlines(), 1):
        m = FENCE.match(line)
        if m:
            marker = m.group(1)[0]
            if in_fence is None:
                in_fence = marker
            elif in_fence == marker:
                in_fence = None
            continue
        if in_fence:
            continue
        scan = INLINE_CODE.sub(lambda mm: " " * len(mm.group(0)), line)
        targets = INLINE_LINK.findall(scan)
        rm = REF_DEF.match(scan)
        if rm:
            targets.append(rm.group(1))
        for t in targets:
            if re.match(r"^[a-z][a-z0-9+.-]*:", t, re.I):
                continue  # external (http, mailto, ...)
            total += 1
            path, _, frag = t.partition("#")
            path = unquote(path)
            if path.startswith("/"):
                broken.append((f, lineno, t, "absolute path"))
                continue
            target = posixpath.normpath(posixpath.join(d, path)) if path else f
            if target.startswith(".."):
                broken.append((f, lineno, t, "escapes repository"))
                continue
            if path and (target == "target" or target.startswith("target/") or is_ignored(target)):
                if LOCAL_LABEL.search(line):
                    local_ok += 1
                    continue
                broken.append((f, lineno, t, "gitignored target not labelled as a local artifact"))
                continue
            if not os.path.exists(target):
                broken.append((f, lineno, t, "missing target"))
                continue
            if frag:
                anchors_checked += 1
                if not target.endswith(".md") or os.path.isdir(target):
                    broken.append((f, lineno, t, "anchor on non-Markdown target"))
                elif unquote(frag) not in anchors_for(target):
                    broken.append((f, lineno, t, "anchor not found"))

for f, n, t, why in broken:
    print(f"BROKEN {f}:{n}: {t} ({why})")
print(f"files={len(files)} relative_links={total} anchors_checked={anchors_checked} "
      f"local_artifact_links={local_ok} broken={len(broken)}")
sys.exit(1 if broken else 0)
