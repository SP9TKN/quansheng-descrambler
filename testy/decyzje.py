"""Zmiany decyzji prawdziwego programu (symuluj) z czasem trwania i siłą dowodów."""
import re, subprocess, sys

sys.stdout.reconfigure(encoding="utf-8")
cli, plik = sys.argv[1:3]
wy = subprocess.run([cli, "symuluj", plik], capture_output=True, text=True, encoding="utf-8").stdout.splitlines()
rx = re.compile(r"\s*([\d.]+) s\s+(.+?) (pewny|niepewny)\s+punkty ([\d.-]+) przewaga ([\d.-]+) ramek\s+(\d+) -> ton (.+)")
ost = None; zm = []
for l in wy:
    m = rx.match(l)
    if m and m[7] != ost:
        ost = m[7]; zm.append((float(m[1]), ost, m[2], float(m[4]), int(m[6])))
tk = float(rx.match(wy[-1])[1]) if wy else 0
print(f"zmian: {len(zm)}")
for i, (t, s, w, p, r) in enumerate(zm):
    k = zm[i + 1][0] if i + 1 < len(zm) else tk
    print(f"  {t:7.1f} s -> {s:<22} przez {k-t:6.1f} s  (punkty {p:.2f}, ramek {r})")
