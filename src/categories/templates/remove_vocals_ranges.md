# media_remove_vocals — placing --from and --to

Vocals are cancelled only BETWEEN --from and --to (Hz); everything below --from and above --to is kept untouched. Aim for a band that spans the voice but spares the instruments around it — bass, kick, and any centre-panned lead are cancellable too, so the edges exist to protect them.

- **--from** — lower edge, `[0..500]`, default **120**. Set it just under the voice's lowest *fundamental* (Table 1) so the bassline stays below the band.
- **--to** — upper edge, `(--from..18000]`, default **none (∞)**. Set it to spare centre-panned cymbals / "air", or leave it off to cancel to the top. A vocal's *pitch* rarely passes ~1.4 kHz, but its brightness (formants, sibilance) reaches far higher — see the caveats.



# Quick orientation, since "frequency" is two different things:

- **Every Hz figure below is the *fundamental* (F0) — i.e., pitch** — unless it's explicitly labelled a *formant/overtone*.
- **Anchors:** human hearing ≈ **20 Hz–20 kHz**; a piano runs **A0 (27.5 Hz) → C8 (4,186 Hz)**; middle C = **C4 = 262 Hz**.



# Caveats

- **Pitch ≠ brightness.** Even a Batman growl and a coloratura soprano both radiate overtones toward ~20 kHz; what makes one "dark" and one "piercing" is *how much energy sits high* (sibilance ~5–10 kHz, "air" 10–20 kHz), which the F0 column doesn't capture. That's why the formant rows matter — and why a low --to can dull a voice even when its *pitch* sits far below the ceiling.
- **These are typical tessituras, not walls.** Speaking F0 rises with excitement or shouting; sung ranges overlap heavily, and voice *type* depends on timbre and comfortable range, not just extremes. Child figures especially vary by individual and study.
- **This is the karaoke (centre-cancellation) trick, not AI source separation.** Reverb-heavy or off-centre vocals only partly cancel, and anything else mixed dead-centre — kick, snare, bass, a centred lead — is exactly what --from and --to exist to protect.
- **Stereo only.** Mono has no centre to subtract; downmix surround to stereo first.



# Table 1 — the pitch continuum (speech 🗣 + singing 🎤), low → high

| Voice / genre (correct term) | Mode | Typical note range | **Fundamental (Hz)** | Notable |
|---|---|---|---|---|
| **Death growl — "Batman sings death metal"** (guttural) | 🎤 | ~C1–C3, indeterminate | **~50–150 Hz** (sounds lower via subharmonics) | False-fold vibration; noise-dominated, not cleanly pitched |
| **Basso profondo / oktavist** (Russian Orthodox) | 🎤 | A1–E4; extremes to C1 | **~55–330 Hz**; down to **33 Hz** | Lowest *true pitched* singing on Earth |
| **Vocal fry / "Batman" gravel** (creak register) | 🗣 | sub-bass creak | **~20–50 Hz** | Irregular pulses; the gritty/authoritative rasp |
| **Deep male / radio-announcer speech** | 🗣 | E2–A2 | **~82–110 Hz** | Bottom of everyday male speech |
| **Average adult male speech** | 🗣 | F2–F3 | **~85–180 Hz** (avg ~115) | Conversational baseline |
| **Male pop / rock / country / crooner** | 🎤 | A2–C5 | **~110–523 Hz** | Baritone–tenor belt |
| **Rap / hip-hop** | 🗣🎤 | F2–F3 | **~85–180 Hz** | *Speech* pitch, *musical* rhythm |
| **Gregorian chant (Latin plainsong)** | 🎤 | C3–D4, ≤ octave | **~131–294 Hz** | Narrow; pitch is relative (a cappella) |
| **Average adult female speech** | 🗣 | E3–C4 | **~165–255 Hz** (avg ~200) | ~1.7× higher than male, from puberty |
| **Young child speech (kindergarten, 4–6)** | 🗣 | ~A3–B3 centre | **~220–255 Hz** avg (spread ~135–300) | Boys ≈ girls — sexes split only at age ~7–9 |
| **Female pop / R&B / soul** | 🎤 | G3–E5 | **~196–659 Hz** | Belts to E5+ |
| **K-pop / J-pop (idol)** | 🎤 | A3–E5 | **~220–659 Hz** | Very bright; boosted "air" 8–16 kHz |
| **Children's choir ("little girl at kindergarten")** | 🎤 | preschool C4–A4; trained to G5 | **~262–440 Hz** (→ **784 Hz**) | Comfortable core E4–G4; narrow ~a fifth when young |
| **Musical-theatre / gospel belt (female)** | 🎤 | up to E5–F5 | up to **~659–698 Hz** | High chest-mix belt |
| **Projected / podium / stage speech** | 🗣 | speech F0 (as above) | + **speaker's/actor's formant ~3–3.7 kHz** | Same pitch, added "ring" so it carries unamplified |
| **Opera — soprano / *coloratura*** | 🎤 | C4–F6 | **~262–1,397 Hz** | + **singer's formant 2.5–3.5 kHz** to cut over the orchestra |
| **Whistle register (Mariah, Minnie Riperton)** | 🎤 | E6–G♯7 | **~1,319–3,322 Hz** | Only the rear vocal-fold edges vibrate; <5% of singers |



# Table 2 — special cases (no single pitch, or synthetic)

| Voice / genre | Frequency behaviour |
|---|---|
| **Whisper** 🗣 | **No fundamental at all** — vocal folds don't vibrate. Turbulent broadband noise shaping the formants; energy mostly **~500 Hz–4 kHz**, rolling off below 500 Hz. |
| **Tuvan throat singing (*khoomei*; *sygyt*/*kargyraa*)** 🎤 | **Biphonic** — a low drone (**~100–150 Hz**, kargyraa lower) *and* a whistling overtone melody (**1–3.5 kHz**) at the same instant. |
| **Vocaloid (e.g., Hatsune Miku)** 🤖 | Optimum **A3–E5 (220–659 Hz)**, extends to ~C6 (1,047 Hz); synthesized, so **no physical floor or ceiling** — quality just degrades outside the band. |
| **Electronic (EDM / synth-pop; *vocoder*, *talkbox*)** 🎛 | "Vocals" are processed samples; the synths themselves span the **full 20 Hz–20 kHz** — sub-bass below ~30 Hz, "air" past 16 kHz. |



# Table 3 — robotic / electronic "vocal" effects 🤖

A processing layer over an ordinary human (or synthetic) source — **none open a new pitch range**. The "robot" cue is either unnaturally *exact pitch* (quantized/monotone) or unnaturally *regular/inharmonic spectrum*.

| Effect (what it does to pitch) | Why it reads as "robotic" | Signature | The frequencies it lands on |
|---|---|---|---|
| **Harmonizer** — forces the sung note to preset scale degrees; stacks pitch-shifted copies | Quantized pitch-shift + stacked harmony voices | Eiffel 65 — "Blue (Da Ba Dee)" | Locked to the track's scale (Blue = G minor: **G3 196 → G4 392 Hz**, stacks to ~**G5 784 Hz**) — a normal tenor octave |
| **Auto-Tune (extreme)** — F0 snapped instantly to the nearest semitone | The mechanical stepping/warble between notes, no natural scoop | Cher — "Believe"; T-Pain | The singer's own range, **quantized to the exact 12-TET semitones** (map below) |
| **Vocoder** — pitch comes wholly from a synth carrier; the voice's own pitch is discarded | A synth "speaking" — inhumanly smooth, no micro-variation | Kraftwerk; Daft Punk | Whatever the carrier plays; the voice adds only a formant envelope (energy to a few kHz) |
| **Talk box** — pitch from an instrument piped through a tube; the mouth shapes formants | A "talking guitar" — organic formants on a non-vocal pitch source | Bon Jovi; Zapp / Roger Troutman | The instrument's pitch (guitar low **E2 = 82 Hz** upward) |
| **Ring modulation** — no pitch tracking; voice × a fixed sine | Metallic/clangy — the spectrum stops being "voice-shaped" | Daleks (*Doctor Who*) | Not on a pitch — voice × a **~30–100 Hz** sine → inharmonic sum/difference sidebands (f ± carrier) |
| **Speech synthesis / TTS** — rule-generated, usually a flat monotone | Fully synthetic — no human voice at all | DECtalk (Stephen Hawking) | Near-monotone low male pitch, **~120 Hz** (approx), formant-synthesised |



# Reference extremes (records, as bookends)

- **Lowest ever:** Tim Storms — **G−7 = 0.189 Hz** (infrasound, inaudible; also holds the ~10-octave widest-range record).
- **Highest *larynx-produced* male note:** Amirhossein Molaei — **F♯8 ≈ 5,989 Hz** (Guinness).
- **Highest reliably documented in a song:** Mariah Carey — **G♯7 = 3,322 Hz** ("Emotions").
- **Contested:** Georgia Brown's "**G10 ≈ 25,087 Hz**" and Adam Lopez's "C♯8" whistle record are both **at or above the 20 kHz hearing limit**, and their note/Hz labels don't reconcile (a real C♯8 is ~4,435 Hz, not the ~14,640 Hz some outlets print) — treat as disputed *(flagged)*.



# Note → frequency map (Hz) ; equal temperament, A4 = 440 Hz

┌─────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┐
│ Oct │    C    │   C♯    │    D    │   D♯    │    E    │    F    │   F♯    │    G    │   G♯    │    A    │   A♯    │    B    │
├─────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┤
│ **0**   │ 16.35   │ 17.32   │ 18.35   │ 19.45   │ 20.60   │ 21.83   │ 23.12   │ 24.50   │ 25.96   │ 27.50   │ 29.14   │ 30.87   │
├─────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┤
│ **1**   │ 32.70   │ 34.65   │ 36.71   │ 38.89   │ 41.20   │ 43.65   │ 46.25   │ 49.00   │ 51.91   │ 55.00   │ 58.27   │ 61.74   │
├─────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┤
│ **2**   │ 65.41   │ 69.30   │ 73.42   │ 77.78   │ 82.41   │ 87.31   │ 92.50   │ 98.00   │ 103.83  │ 110.00  │ 116.54  │ 123.47  │
├─────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┤
│ **3**   │ 130.81  │ 138.59  │ 146.83  │ 155.56  │ 164.81  │ 174.61  │ 185.00  │ 196.00  │ 207.65  │ 220.00  │ 233.08  │ 246.94  │
├─────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┤
│ **4**   │ 261.63  │ 277.18  │ 293.66  │ 311.13  │ 329.63  │ 349.23  │ 369.99  │ 392.00  │ 415.30  │ 440.00  │ 466.16  │ 493.88  │
├─────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┤
│ **5**   │ 523.25  │ 554.37  │ 587.33  │ 622.25  │ 659.26  │ 698.46  │ 739.99  │ 783.99  │ 830.61  │ 880.00  │ 932.33  │ 987.77  │
├─────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┤
│ **6**   │ 1046.50 │ 1108.73 │ 1174.66 │ 1244.51 │ 1318.51 │ 1396.91 │ 1479.98 │ 1567.98 │ 1661.22 │ 1760.00 │ 1864.66 │ 1975.53 │
├─────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┤
│ **7**   │ 2093.00 │ 2217.46 │ 2349.32 │ 2489.02 │ 2637.02 │ 2793.83 │ 2959.96 │ 3135.96 │ 3322.44 │ 3520.00 │ 3729.31 │ 3951.07 │
├─────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┤
│ **8**   │ 4186.01 │ 4434.92 │ 4698.64 │ 4978.03 │ 5274.04 │ 5587.65 │ 5919.91 │ 6271.93 │ 6644.88 │ 7040.00 │ 7458.62 │ 7902.13 │
└─────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┘

**Formula (extend it anywhere):** `f = 440 × 2^((n − 69) / 12)`, where `n` is the MIDI note number (A4 = 69, C4 = 60). Up an octave = ×2; up one semitone = ×2^(1/12) ≈ 1.05946.

**Anchors & sanity checks:** middle C = **C4 = 261.63**; piano spans **A0 (27.50) → C8 (4,186.01)**; hearing ≈ **20 Hz–20 kHz** (so the C0 row is near the very bottom of audibility, and B8 is bright treble). Cross-checks with the earlier tables: Mariah's whistle **G♯7 = 3,322.44 Hz** and coloratura **F6 = 1,396.91 Hz** both fall straight out of this table; Tim Storms' 0.189 Hz record sits ~7 octaves *below* C0 (off the bottom).

