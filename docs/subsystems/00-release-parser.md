# Release parser

Shared utility that extracts structured metadata from torrent release title strings. Used by Search (to parse indexer results), Import (to tag media files), and Quality Scorer (to compare releases).

This is the highest-risk shared component in kino — incorrect parsing leads to wrong quality scoring, bad import matching, and missed upgrades.

## Approach

Single-pass tokenizer, not a regex waterfall. Split the title into tokens, look up each token against a known vocabulary, and extract structured fields.

**Why not regexes:** the obvious approach — accumulate ordered regexes per release-name pattern — is fragile, order-dependent, and grows pathologically with edge cases. A tokenizer is simpler to reason about, test, and extend.

## Pipeline

```
Input: "The.Matrix.1999.Remastered.2160p.UHD.BluRay.x265-TERMiNAL"
  ↓
1. Normalize: strip website prefixes/suffixes, normalize brackets
  ↓
2. Tokenize: split on [._-\s]+ → ["The", "Matrix", "1999", "Remastered", "2160p", "UHD", "BluRay", "x265", "TERMiNAL"]
  ↓
3. Classify: look up each token against known vocabulary
  → "1999" = year, "Remastered" = edition, "2160p" = resolution, "UHD" = source_hint, "BluRay" = source, "x265" = codec
  ↓
4. Extract title: everything before the first metadata token (year for movies, S01E01 for TV)
  → title = "The Matrix"
  ↓
5. Extract release group: last token after a hyphen
  → group = "TERMiNAL"
  ↓
6. Resolve ambiguities: context rules for tokens like "DL", "BD", "WEB"
  ↓
Output: ParsedRelease struct
```

## Output struct

```
ParsedRelease {
  // Content identification
  title: String,
  year: Option<u16>,                    // Movies
  season: Option<u16>,                  // TV
  episodes: Vec<u16>,                   // TV (multiple for S01E01E02)
  is_season_pack: bool,

  // Quality
  resolution: Option<Resolution>,       // _480p, _720p, _1080p, _2160p
  source: Option<Source>,               // Bluray, WebDL, WEBRip, HDTV, etc.
  video_codec: Option<VideoCodec>,      // H264, H265, AV1, etc.
  audio_codec: Option<AudioCodec>,      // AAC, AC3, DTS, TrueHD, Atmos, etc.
  hdr_format: Option<HdrFormat>,        // HDR10, HDR10Plus, DolbyVision, HLG

  // Flags
  is_remux: bool,
  is_proper: bool,
  is_repack: bool,
  is_remastered: bool,

  // Metadata
  release_group: Option<String>,
  languages: Vec<String>,              // ISO 639-1 codes
  edition: Option<String>,             // "Director's Cut", "Extended", etc.
}
```

## Token vocabulary

### Resolution

| Token(s) | Value | Notes |
|---|---|---|
| `360p` | 360p | |
| `480p`, `480i`, `640x480`, `848x480` | 480p | |
| `540p` | 540p | |
| `576p` | 576p | |
| `720p`, `1280x720` | 720p | |
| `1080p`, `1080i`, `1920x1080`, `FHD` | 1080p | |
| `2160p`, `3840x2160`, `4K`, `UHD` | 2160p | `UHD` alone implies 2160p |

**Ambiguity:** `4K` and `UHD` can appear as source hints too. If `2160p` is already found, ignore them as resolution. If no resolution found, treat them as 2160p.

### Source

| Token(s) | Value | Notes |
|---|---|---|
| `BluRay`, `Blu-Ray`, `BDRip`, `BRRip`, `BD` | Bluray | `BD` not at end of string |
| `Remux`, `BD Remux`, `UHD Remux` | Bluray + is_remux flag | |
| `WEB-DL`, `WEBDL`, `WEB DL` | WebDL | |
| `WEBRip`, `Web-Rip` | WEBRip | |
| `WEB` (standalone, end of string) | WebDL | Context-dependent, case-sensitive |
| `HDTV`, `PDTV`, `DSR`, `SDTV`, `TVRip` | HDTV | |
| `DVDRip`, `DVD`, `DVDR` | DVD | |
| `CAM`, `CAMRIP`, `HDCAM` | Cam | |
| `TS`, `TELESYNC`, `HDTS` | Telesync | |
| `TC`, `TELECINE`, `HDTC` | Telecine | |
| `SCR`, `SCREENER`, `DVDSCR` | Screener | |

**Multi-token:** `WEB-DL` is one token after splitting on spaces only (hyphen preserved). Or if split on hyphens too, look for `WEB` followed by `DL` and combine.

**Ambiguity:** `DL` means "dual language" in German releases. Only treat as part of `WEB-DL` if preceded by `WEB`. A standalone `DL` after a German language indicator means dual-language.

### Video codec

| Token(s) | Value |
|---|---|
| `x264`, `h264`, `H.264`, `AVC` | H264 |
| `x265`, `h265`, `H.265`, `HEVC` | H265 |
| `AV1` | AV1 |
| `VP9` | VP9 |
| `XviD`, `DivX` | XviD |
| `MPEG2`, `MPEG-2` | MPEG2 |
| `VC-1`, `VC1` | VC1 |

### Audio codec

| Token(s) | Value |
|---|---|
| `AAC`, `AAC2.0`, `AAC5.1` | AAC |
| `AC3`, `DD5.1`, `DD2.0`, `Dolby Digital` | AC3 |
| `EAC3`, `DDP`, `DDP5.1`, `DDP2.0`, `DD+`, `Dolby Digital Plus` | EAC3 |
| `DTS` | DTS |
| `DTS-HD`, `DTS-HD MA`, `DTS-HD.MA`, `DTSHDMA` | DTSHD |
| `TrueHD` | TrueHD |
| `Atmos` | Atmos (modifier on TrueHD or EAC3) |
| `FLAC` | FLAC |
| `Opus` | Opus |
| `LPCM`, `PCM` | LPCM |

**Multi-token:** `DTS-HD MA` is two tokens. `DDP5.1` combines codec and channel info. Handle by peeking at adjacent tokens.

### HDR format

| Token(s) | Value |
|---|---|
| `HDR`, `HDR10` | HDR10 |
| `HDR10+`, `HDR10Plus` | HDR10Plus |
| `DV`, `DoVi`, `Dolby Vision`, `DolbyVision` | DolbyVision |
| `HLG`, `HLG10` | HLG |
| `SDR` | SDR |

**Note:** parsing HDR from release names lets us score a torrent before downloading it. The alternative — waiting for the file to land and reading mediainfo — is too late for grab-time decisions.

### Languages

| Token(s) | Language |
|---|---|
| `MULTi`, `MULTi-` | Multiple (accept, don't filter) |
| `DUAL`, `DL` (after German indicator) | Dual language |
| `FRENCH`, `TRUEFRENCH`, `VFF`, `VFQ`, `VF` | French |
| `GERMAN`, `DEUTSCH` | German |
| `SPANISH`, `LATINO`, `CASTELLANO` | Spanish |
| `ITALIAN`, `ITA` | Italian |
| `PORTUGUESE`, `DUBLADO` | Portuguese |
| `RUSSIAN`, `RUS` | Russian |
| `JAPANESE`, `JAP`, `JPN` | Japanese |
| `KOREAN`, `KOR` | Korean |
| `CHINESE`, `CHI`, `CHS`, `CHT` | Chinese |
| `HINDI` | Hindi |
| `TURKISH`, `TUR` | Turkish |
| `POLISH`, `PL`, `PLDUB` | Polish |
| `DUTCH`, `NL` | Dutch |
| `SWEDISH`, `SWE` | Swedish |
| `NORDIC`, `NORSE` | Nordic (multiple Scandinavian) |

If no language token found, the release is assumed to match the user's preferred language (most releases are English with no explicit tag).

### Flags / modifiers

| Token(s) | Flag |
|---|---|
| `PROPER` | is_proper |
| `REPACK`, `RERIP` | is_repack |
| `REMASTERED` | is_remastered |
| `INTERNAL` | internal (from indexer flags, not usually in title) |
| `COMPLETE` | is_season_pack (for TV) |
| `UNCUT`, `UNRATED`, `UNCENSORED` | edition flag |
| `EXTENDED` | edition flag |
| `DC`, `Directors Cut`, `Director's Cut` | edition = "Director's Cut" |
| `IMAX` | edition = "IMAX" |
| `DUBBED`, `DUB` | dubbed (not original language audio) |
| `SUBBED`, `SUB`, `HARDSUB`, `HC` | hardcoded subtitles |

### TV patterns

| Pattern | Extraction |
|---|---|
| `S01E05` | season=1, episodes=[5] |
| `S01E05E06` | season=1, episodes=[5,6] |
| `S01E05-E08` | season=1, episodes=[5,6,7,8] |
| `S01` (no episode) | season=1, is_season_pack=true |
| `1x05` | season=1, episodes=[5] |
| `Season 1` or `Season.1` | season=1, is_season_pack=true |

### Movie year

Pattern: 4-digit number matching `(19|20)\d{2}` that is NOT followed by `p` or `i` (to avoid matching `1080p`, `2160p`).

## Title extraction

Everything before the first recognized metadata token is the title:

```
"The.Matrix.1999.Remastered.2160p.UHD.BluRay.x265-TERMiNAL"
 ^^^^^^^^^^ ^^^^
 title      year (first metadata token)
```

For TV:
```
"Breaking.Bad.S05E14.Ozymandias.1080p.BluRay.x264-DON"
 ^^^^^^^^^^^^       ^^^^^^^^^^^ <- episode title (between S01E01 and quality tokens)
```

The episode title (text between the season/episode pattern and the first quality token) is also extracted when present.

## Release group

The last segment after a hyphen, if it doesn't match any known token:

```
"The.Matrix.1999.2160p.BluRay.x265-TERMiNAL"
                                    ^^^^^^^^^ group = "TERMiNAL"
```

Strip any trailing bracket content: `TERMiNAL[rarbg]` → `TERMiNAL`.

## Edge cases

- **Year in show name:** "2001 A Space Odyssey" — the year 2001 is part of the title, not the release year. Handle by checking if a second year follows (the release year).
- **Numbers in show names:** "S.W.A.T." contains periods that look like delimiters. Pre-processing should handle known abbreviations.
- **No metadata tokens:** some releases have minimal naming. Fall back to returning just the title with no quality info.
- **Concatenated tokens:** `bluray1080p` (no separator). Handle as a fallback lookup.
- **Case sensitivity:** `REAL` is case-sensitive (only uppercase). Most other tokens are case-insensitive.

## Testing

The parser must have comprehensive test coverage. Priority test categories:

1. Standard movie releases (title + year + quality)
2. Standard TV releases (show + S01E01 + quality)
3. Season packs (S01 COMPLETE)
4. Multi-episode files (S01E01E02)
5. Remux releases
6. HDR releases (HDR10, DV, HDR10+)
7. Multi-language releases (MULTi, DL)
8. Anime-style naming (basic only, no absolute numbering)
9. Releases with no quality info
10. Edge cases (year in title, numbers in show name, concatenated tokens)
