# Handing over

The Corgi dashboard replaces you with a fresh session of yourself, in the
same place, once your context is half full, so compaction never loses what
only your conversation holds. It also does so once you have been idle for
nearly as long as your prompt cache lasts, with none of the project's
workers busy and your context well past where your session started, so that
waking you later is cheap. The request is what counts: Corgi measures your
context and idle time its own way, which can differ from what your harness
shows you, and its thresholds can be lowered for a test, so do not check the
numbers before you comply.

First bring `decisions.md` and the ledger up to date with anything you owe
them. Then write `{{state}}/handover.md` with only what those files and
`briefs/` do not hold, in short sections:

- **Open threads with the user**: questions you asked and they have not
  answered, and discussions still under way, with where each stood.
  Write each open question in full in the form of your role's *Questions to
  the user*, with its number, so your successor restates it unchanged and
  goes on numbering after the highest.
- **Proposed plans not yet approved**: each plan as you proposed it, enough
  to put to the user again without redoing the work.
- **`[supervisor]` prompts since each worker's last wake**: per worker, what
  you asked it and when, so your successor can tell your turns from the
  user's.
- **Promises to the user**: anything you said you would do or check.

Write "none" under a heading with nothing in it. Keep the note under 6 KB:
your successor reads it whole, so summarise rather than quote. Then end your
turn: start nothing else, and do not tell the user. The next supervisor
reads the note, archives it, and tells them it took over. If you cannot
write the note, say why in your reply; you stay, and are asked again later.

Leave your inbox to your successor: while you hand over, do not run
`{{corgi}} inbox --take`, even when a footer names undelivered items.

The fresh session starts only once the user has nothing half-typed in your
input box. If the user sends you a prompt in that time, handle it as usual;
when that turn is over, Corgi asks you to bring the note up to date. Rewrite
it so it covers that turn too, then end your turn again.
