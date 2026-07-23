# media_remove_vocals — placing --from and --to

Vocals are cancelled only BETWEEN --from and --to (Hz); everything below --from and above --to is kept untouched. Aim for a band that spans the voice but spares the instruments around it — bass, kick, and any centre-panned lead are cancellable too, so the edges exist to protect them.

- **--from** — lower edge, `[0..500]`, default **120**. Set it just under the voice's lowest *fundamental* (Table 1) so the bassline stays below the band.
- **--to** — upper edge, `(--from..18000]`, default **none (∞)**. Set it to spare centre-panned cymbals / "air", or leave it off to cancel to the top. A vocal's *pitch* rarely passes ~1.4 kHz, but its brightness (formants, sibilance) reaches far higher — see the caveats.

Quick orientation, since "frequency" is two different things:
- **Every Hz figure below is the *fundamental* (F0) — i.e., pitch** — unless it's explicitly labelled a *formant/overtone*.
- **Anchors:** human hearing ≈ **20 Hz–20 kHz**; a piano runs **A0 (27.5 Hz) → C8 (4,186 Hz)**; middle C = **C4 = 262 Hz**.

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

# Reference extremes (records, as bookends)

- **Lowest ever:** Tim Storms — **G−7 = 0.189 Hz** (infrasound, inaudible; also holds the ~10-octave widest-range record).
- **Highest *larynx-produced* male note:** Amirhossein Molaei — **F♯8 ≈ 5,989 Hz** (Guinness).
- **Highest reliably documented in a song:** Mariah Carey — **G♯7 = 3,322 Hz** ("Emotions").
- **Contested:** Georgia Brown's "**G10 ≈ 25,087 Hz**" and Adam Lopez's "C♯8" whistle record are both **at or above the 20 kHz hearing limit**, and their note/Hz labels don't reconcile (a real C♯8 is ~4,435 Hz, not the ~14,640 Hz some outlets print) — treat as disputed *(flagged)*.

# Caveats

- **Pitch ≠ brightness.** Even a Batman growl and a coloratura soprano both radiate overtones toward ~20 kHz; what makes one "dark" and one "piercing" is *how much energy sits high* (sibilance ~5–10 kHz, "air" 10–20 kHz), which the F0 column doesn't capture. That's why the formant rows matter — and why a low --to can dull a voice even when its *pitch* sits far below the ceiling.
- **These are typical tessituras, not walls.** Speaking F0 rises with excitement or shouting; sung ranges overlap heavily, and voice *type* depends on timbre and comfortable range, not just extremes. Child figures especially vary by individual and study.
- **This is the karaoke (centre-cancellation) trick, not AI source separation.** Reverb-heavy or off-centre vocals only partly cancel, and anything else mixed dead-centre — kick, snare, bass, a centred lead — is exactly what --from and --to exist to protect.
- **Stereo only.** Mono has no centre to subtract; downmix surround to stereo first.
