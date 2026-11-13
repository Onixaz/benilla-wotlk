"""Lays a stretch run's filmstrip along its steps: scripts/stretch_report.py <run dir>.

Reads the run's trace.txt (the `step`, `shot` and `audit` lines) and writes index.html beside the
shots: one row per step with its shots, and every wire-audit line where it happened.
"""

import html
import os
import re
import sys


def main(run):
    trace = os.path.join(run, "trace.txt")
    rows = []  # (t, kind, text)
    if os.path.exists(trace):
        with open(trace, encoding="utf-8", errors="replace") as f:
            for line in f:
                m = re.match(r"t=\s*([\d.]+)\s+(step|shot|audit)\s+(.*)", line.rstrip("\n"))
                if m:
                    rows.append((float(m.group(1)), m.group(2), m.group(3)))
    verdict = ""
    log = os.path.join(run, "client.log")
    if os.path.exists(log):
        with open(log, encoding="utf-8", errors="replace") as f:
            for line in f:
                line = re.sub(r"\x1b\[[0-9;]*m", "", line)
                if "stretch-verdict:" in line:
                    verdict = line.split("stretch-verdict:", 1)[1].strip()
    name = os.path.basename(os.path.normpath(run))
    out = [
        "<!doctype html><meta charset=utf-8><title>%s</title>" % html.escape(name),
        "<style>body{font:14px system-ui;background:#111;color:#ddd;margin:16px}"
        "h1{font-size:18px}.step{margin:18px 0 6px;font-weight:600;color:#fff}"
        ".audit{color:#f77;font-family:monospace;margin:2px 0}"
        ".shots{display:flex;flex-wrap:wrap;gap:6px}"
        ".shots figure{margin:0}.shots img{width:320px;border:1px solid #333}"
        ".shots figcaption{font-size:11px;color:#999}</style>",
        "<h1>%s</h1><p>%s</p>" % (html.escape(name), html.escape(verdict or "no verdict")),
    ]
    open_shots = False
    for t, kind, text in rows:
        if kind == "step":
            if open_shots:
                out.append("</div>")
            out.append('<div class=step>%.1f s — step %s</div><div class=shots>' % (t, html.escape(text)))
            open_shots = True
        elif kind == "shot":
            path = text.split(" ")[0]
            if not open_shots:
                out.append("<div class=shots>")
                open_shots = True
            rel = os.path.basename(path)
            out.append(
                '<figure><a href="%s"><img loading=lazy src="%s"></a><figcaption>%.1f s %s</figcaption></figure>'
                % (html.escape(rel), html.escape(rel), t, html.escape(rel))
            )
        else:
            if open_shots:
                out.append("</div>")
                open_shots = False
            out.append("<div class=audit>%.1f s %s</div>" % (t, html.escape(text)))
    if open_shots:
        out.append("</div>")
    with open(os.path.join(run, "index.html"), "w", encoding="utf-8") as f:
        f.write("\n".join(out))


if __name__ == "__main__":
    main(sys.argv[1])
