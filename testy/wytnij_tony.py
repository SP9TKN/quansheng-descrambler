"""Wycina z nagrania nadania zawierające sygnały tonowe. Bez numeru kanału: rozpoznanie po kształcie sygnału.
wytnij_tony.py <folder_wyjsciowy> <nagranie.wav>...   Czas zegarowy: koniec nagrania = czas modyfikacji pliku."""
import csv, datetime, os, sys, wave
import numpy as np
from scipy.signal import resample_poly

sys.stdout.reconfigure(encoding="utf-8")
WYJ = sys.argv[1]
WZORCE = {
    "beep_800-600-700-800": [(800, .055), (600, .055), (700, .055), (800, .05)],
    "beep_600-800-1000": [(600, .047), (800, .047), (1000, .047)],
}
HOP = .005


def czytaj(p):
    w = wave.open(p)
    x = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(float) / 32768
    return x, w.getframerate()


def zapisz(p, x, fs):
    w = wave.open(p, "wb"); w.setnchannels(1); w.setsampwidth(2); w.setframerate(fs)
    w.writeframes((np.clip(x, -1, 1) * 32767).astype(np.int16).tobytes()); w.close()


def subton(x, fs):
    if len(x) < fs // 2:
        return ""
    o = x[:fs * 8]
    fr = np.fft.rfftfreq(len(o), 1 / fs); sp = np.abs(np.fft.rfft(o * np.hanning(len(o)))) * 2 / len(o)
    z = (fr >= 60) & (fr <= 260)
    return f"{fr[z][np.argmax(sp[z])]:.1f}" if sp[z].max() > .0004 else ""


def analiza(x8, fs8):
    dl = int(fs8 * .02); hop = int(fs8 * HOP); NFFT = 1024
    n = (len(x8) - dl) // hop
    fr = np.fft.rfftfreq(NFFT, 1 / fs8)
    z = (fr >= 300) & (fr <= 3500)
    okno = np.hanning(dl)
    P = np.empty((n, NFFT // 2 + 1))
    for i in range(n):
        P[i] = np.abs(np.fft.rfft(x8[i * hop: i * hop + dl] * okno, NFFT)) ** 2
    calk = P[:, z].sum(axis=1) + 1e-15
    return P, calk, fr


def znajdz(P, calk, fr, wzorzec, prog=.55):
    """Na każdej pozycji: udział mocy kolejnych tonów w ich odcinkach czasu."""
    n = len(calk); wyn = np.ones(n)
    t = 0.0
    for f, d in wzorzec:
        z = (fr >= f - 30) & (fr <= f + 30)
        u = P[:, z].sum(axis=1) / calk
        a = int((t + .012) / HOP); b = int((t + d - .012) / HOP)
        k = np.ones(max(1, b - a)) / max(1, b - a)
        sr = np.convolve(u, k, "full")[b - a - 1:]  # średnia od pozycji i
        s = np.zeros(n); s[: n - a] = sr[a: n] if len(sr) >= n else np.pad(sr[a:], (0, n - len(sr)))[: n - a]
        wyn = np.minimum(wyn, s); t += d
    energia = np.sqrt(calk)
    trafienia = []
    i = 0; prog_e = max(np.percentile(energia, 50) * 3, .02)
    while i < n:
        if wyn[i] >= prog and energia[min(n - 1, i + int(.03 / HOP))] > prog_e:
            j = i + int(np.argmax(wyn[i: i + 20]))
            trafienia.append((j * HOP, float(wyn[j]))); i = j + int(.5 / HOP)
        else:
            i += 1
    return trafienia


def dlugie_tony(P, calk, fr):
    """Długie sygnały z dominującą jedną linią: alarm, tony naprzemienne."""
    z = (fr >= 300) & (fr <= 3500); fz = fr[z]; Pz = P[:, z]
    j = Pz.argmax(axis=1); ide = np.arange(len(j))
    dom = np.array([Pz[i, max(0, j[i] - 8): j[i] + 9].sum() for i in ide]) / calk
    en = np.sqrt(calk); ton = (dom > .7) & (en > max(np.percentile(en, 50) * 3, .02))
    wyn = []; idx = np.flatnonzero(ton)
    if not len(idx):
        return wyn
    st = pop = idx[0]
    for i in list(idx[1:]) + [idx[-1] + 10 ** 6]:
        if i - pop > 16:
            if (pop - st) * HOP >= .6:
                wyn.append((st * HOP, (pop - st) * HOP, sorted({int(round(v / 50) * 50) for v in fz[j[st:pop + 1]]})))
            st = i
        pop = i
    return wyn


def nadanie(x, fs, t):
    """Granice nadania wokół t: dłuższa niż 0,4 s cisza kończy nadanie."""
    bl = int(fs * .02); e = np.sqrt(np.convolve(x**2, np.ones(bl) / bl, "same"))[:: bl]
    ak = e > .0015; n = len(ak); i = min(n - 1, int(t * fs / bl))
    przerwa = int(.4 / .02)
    a = i
    while a > 0 and ak[max(0, a - przerwa): a].any():
        a -= 1
    b = i
    while b < n - 1 and ak[b: b + przerwa].any():
        b += 1
    return max(0, a * bl - int(.2 * fs)), min(len(x), b * bl + int(.2 * fs))


lista = []
for pl in sys.argv[2:]:
    x, fs = czytaj(pl)
    koniec = datetime.datetime.fromtimestamp(os.path.getmtime(pl))
    zero = koniec - datetime.timedelta(seconds=len(x) / fs)
    x8 = resample_poly(x, 1, fs // 8000); P, calk, fr = analiza(x8, 8000)
    zdarzenia = []
    for naz, wz in WZORCE.items():
        zdarzenia += [(t, sum(d for _, d in wz), naz, s) for t, s in znajdz(P, calk, fr, wz)]
    for t, d, f in dlugie_tony(P, calk, fr):
        if len(f) >= 1:
            zdarzenia.append((t, d, "dlugi_ton_" + "-".join(map(str, f[:4])), 1.0))
    zdarzenia.sort()
    zrobione = {}
    for t, d, naz, sk in zdarzenia:
        a, b = nadanie(x, fs, t + d / 2)
        kl = (naz, a)
        kat = os.path.join(WYJ, naz if naz.startswith("beep_800") else os.path.join("inne_sygnaly", naz))
        os.makedirs(kat, exist_ok=True)
        zegar = (zero + datetime.timedelta(seconds=t)).strftime("%H-%M-%S")
        if kl in zrobione:
            zrobione[kl] += 1
        else:
            zrobione[kl] = 1
            zapisz(os.path.join(kat, f"{zegar}_nadanie.wav"), x[a:b], fs)
        c = int(.1 * fs)
        zapisz(os.path.join(kat, f"{zegar}_sam_sygnal.wav"), x[max(0, int(t * fs) - c): int((t + d) * fs) + c], fs)
        lista.append(dict(zegar=zegar.replace("-", ":"), sygnal=naz, dl_sygnalu_s=round(d, 2), dl_nadania_s=round((b - a) / fs, 1),
                          subton_hz=subton(x[a:b], fs), zgodnosc=round(sk, 2), plik=os.path.basename(pl), t_s=round(t, 2)))
    print(os.path.basename(pl), "zdarzeń:", len(zdarzenia))
sc = os.path.join(WYJ, "lista.csv")
stare = list(csv.DictReader(open(sc, encoding="utf-8-sig"))) if os.path.exists(sc) else []
klucz = {(r["plik"], r["sygnal"], str(r["t_s"])) for r in stare}
nowe = [r for r in lista if (r["plik"], r["sygnal"], str(r["t_s"])) not in klucz]
wsz = stare + nowe
with open(sc, "w", newline="", encoding="utf-8-sig") as f:
    c = csv.DictWriter(f, fieldnames=list(lista[0].keys()), delimiter=";"); c.writeheader(); c.writerows(wsz)
for r in lista:
    print(r["zegar"], r["sygnal"], "nadanie", r["dl_nadania_s"], "s, subton", r["subton_hz"], "zgodność", r["zgodnosc"])
