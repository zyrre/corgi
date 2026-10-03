# Corgi card mockups: ranking

All mockups are drawn by the dashboard's own code (`src/ui/agents.rs` behind
a test-only `CardStyle` switch) from the README's made-up herd, in Tokyo
Night and Tokyo Night Day. Regenerate them with
`cargo test --lib mockups -- --ignored`. `▙▟` in coat orange stands in for
the sibling task's pixel-art icon.

1. **Lead panel (7).** The strongest separation that works in both themes.
   The corgi's rows sit on the theme's black panel tone, the same tone the
   dialogs use, so the band follows the theme: darker than the background
   in Tokyo Night, lighter in Tokyo Night Day. A coat-orange tab runs down
   the left, the icon and coat-orange name sit in the title, and the workers
   hang from it as a dimmed tree. The selected worker and anything blocked
   keep full colour. Weak spot: the panel tone alone is subtle in Tokyo
   Night, so the tab and the title do most of the work.
2. **Heavy section (6).** The clearest at a glance and the cheapest to
   build. A heavy coat-orange box round the corgi meets a light muted box
   round the workers at `┡━┩`. It uses only border glyphs and no background
   colour, so it reads the same in both themes. Pairs well with Tree.
3. **Tree (4).** Pure layout with no colour, so it says "these belong to
   that one" more than "look here". Worker rows give up 4 columns. It
   combines with any of the others, and Lead already includes it.
4. **Coat badge (5).** Very recognisable, but the corgi's state moves out
   of the coloured badge into a small mark. A working corgi then reads
   quieter than its workers, and the pill repeats the "corgi" task name.
5. **Rail (2).** Cheap and tidy, but too quiet on its own; the rail is
   easy to mistake for a selection bar.
6. **Ember frame (3).** The orange border marks the card rather than the
   corgi. Muting the workers hides what you expanded the card to read.
7. **Band (1).** The nicest in the dark theme, but it needs a fixed RGB
   tint mixed against a known background. In Tokyo Night Day it becomes a
   dark brown slab with unreadable text. Corgi cannot know the terminal's
   background colour without an OSC 11 query.

My recommendation: Lead panel, or Heavy section + Tree if Lead's dimming
feels like too much.
