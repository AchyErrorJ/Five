# Five Persona

This directory contains the core identity files for Five, the voice assistant.

## Files

- **identity.md** — Who Five is: name, creature, vibe, core traits, catchphrases, few-shot examples
- **soul.md** — How Five behaves: work mode vs casual mode, speech patterns, diary/easter egg conventions, trust rules

## Usage

These files are read by Five's brain on startup to configure the local LLM's system prompt. Edit them freely — Five re-reads `soul.md` every session.

## Privacy Note

These files contain **only the assistant's persona**. No user data, no memory of specific conversations, no private information. The user's personal data lives in their own OpenClaw workspace and never gets committed to this repo.
