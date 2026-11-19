import sys, glob, os, numpy as np
from scipy.io import wavfile
from scipy import signal
SYNC = 0x7CD215D8; IDLE = 0x7A89C197
def bch_ok(cw):
    v = cw >> 1
    for i in range(30, 9, -1):
        if v & (1 << i): v ^= 0x769 << (i - 10)
    return v == 0 and bin(cw).count('1') % 2 == 0
def bch_fix(cw):
    if bch_ok(cw): return cw, 0
    for i in range(32):
        c = cw ^ (1 << i)
        if bch_ok(c): return c, 1
    for i in range(32):
        for j in range(i+1, 32):
            c = cw ^ (1 << i) ^ (1 << j)
            if bch_ok(c): return c, 2
    return None, 9
def wbits(w): return [(w >> (31-i)) & 1 for i in range(32)]
def load(fn):
    sr, x = wavfile.read(fn); x = x.astype(float)/32768
    sos = signal.butter(4, 1600, 'lowpass', fs=sr, output='sos')
    return sr, signal.sosfiltfilt(sos, x)
D = 8   # decimation step (samples)
L = 3   # +-bits window
def feats(y, centers, sr, rate):
    spb = sr/rate; half = int(L*spb)
    offs = np.arange(-half, half+1, D)
    idx = (centers[:, None] + offs[None, :]).astype(int)
    idx = np.clip(idx, 0, len(y)-1)
    F = y[idx]
    return np.hstack([F, np.ones((len(F), 1))])
def centers_of(n, sr, rate, ph):
    spb = sr/rate
    return ph + np.arange(int((n - ph)/spb)) * spb
def find_sync(b, maxerr):
    s = 0; hits = []
    for k in range(len(b)):
        s = ((s << 1) | int(b[k])) & 0xFFFFFFFF
        if k >= 31:
            e = bin(s ^ SYNC).count('1')
            if e <= maxerr: hits.append((k-31, e))
    return hits
d = sys.argv[1]
files = sorted(glob.glob(os.path.join(d, '*.wav')))
# --- training on file with SYNC + 16 idle (12-55-45), timing from brute force earlier: rate 512
tr = [f for f in files if '12-55-45' in f][0]
sr, y = load(tr)
best = None
for rate in np.arange(511.0, 513.01, 0.1):
    for ph in np.arange(0, sr/rate, 2):
        c = centers_of(len(y), sr, rate, ph)
        b = (y[c.astype(int)] > 0).astype(int)
        h = find_sync(b, 4)
        if not h: continue
        k = h[0][0]
        ref = wbits(SYNC) + wbits(IDLE)*16
        seg = b[k:k+len(ref)]
        if len(seg) < len(ref): continue
        err = np.sum(seg != np.array(ref))
        if best is None or err < best[0]: best = (err, rate, ph, k)
err, rate, ph, k = best
print(f"trening: {os.path.basename(tr)} rate={rate:.1f} ph={ph} sync@{k} bledy przed korekcja={err}/544")
c = centers_of(len(y), sr, rate, ph)[k:k+544]
ref = np.array(wbits(SYNC) + wbits(IDLE)*16)*2-1
F = feats(y, c, sr, rate)
w, *_ = np.linalg.lstsq(F, ref, rcond=None)
print("bledy po korekcji (trening):", np.sum(np.sign(F@w) != ref))
# --- apply to all files
for fn in files:
    sr, y = load(fn)
    best = None
    for rate in np.arange(511.0, 513.51, 0.1):
        for ph in np.arange(0, sr/rate, 3):
            c = centers_of(len(y), sr, rate, ph)
            z = feats(y, c, sr, rate) @ w
            b = (z > 0).astype(int)
            h = find_sync(b, 3)
            if not h: continue
            # score = number of BCH-valid words after each sync
            nok = 0
            for k0, e in h:
                for m in range(17):
                    kk = k0 + 32*m
                    if kk+32 > len(b): break
                    ww = int(''.join(map(str, b[kk:kk+32])), 2)
                    nok += bch_ok(ww)
            sc = (nok, -abs(rate-512))
            if best is None or sc > best[0]: best = (sc, rate, ph, h, b)
    if best is None:
        print(f"\n== {os.path.basename(fn)}: brak sync"); continue
    sc, rate, ph, h, b = best
    print(f"\n== {os.path.basename(fn)} rate={rate:.1f} sync(bit,bledy)={h} poprawnych BCH={sc[0]}")
    # dedupe syncs at 544-bit spacing
    for k0, e in h:
        print(f"  -- paczka od bitu {k0} (t={(ph + k0*sr/rate)/sr:.2f}s)")
        for m in range(1, 17):
            kk = k0 + 32*m
            if kk+32 > len(b): break
            raw = int(''.join(map(str, b[kk:kk+32])), 2)
            fx, ne = bch_fix(raw)
            frame = (m-1)//2
            if fx is None:
                print(f"   {m:2d} ramka{frame} {raw:08X} NIEPOPRAWNE"); continue
            if fx == IDLE: t = "IDLE"
            elif fx >> 31 == 0: t = f"ADRES RIC={(((fx>>13)&0x3FFFF)<<3)|frame} funkcja={(fx>>11)&3}"
            else: t = f"WIAD dane={(fx>>11)&0xFFFFF:05X}"
            print(f"   {m:2d} ramka{frame} {fx:08X} popr={ne} {t}")
