// Corgi's status plugin for OpenCode (io.github.zyrre.corgi).
//
// herdr-core (hc), the Herdr app's backend, learns what a program in one of
// its terminals is doing from OSC 7501 reports, which OpenCode does not
// write. This plugin writes them from OpenCode's own events, so hc sees an
// OpenCode session work, rest and wait on a question: Corgi's daemon can
// then wake a supervisor when an OpenCode worker stops, and hc refuses to
// type into OpenCode's permission dialog, where Enter would answer it.
//
// It writes only inside an hc terminal ($HC_TERMINAL_ID), to the terminal
// itself (/dev/tty), and never fails OpenCode: a report that cannot be
// written is dropped. herdr-app/install.sh installs it into
// ~/.config/opencode/plugin/ after asking, and removes it when OpenCode
// reports by itself. Tested with OpenCode 1.18.33 (docs/herdr-app-port.md).
import { writeFileSync } from "node:fs";

const inHc = Boolean(process.env.HC_TERMINAL_ID);

const say = (state, extra = "") => {
  if (!inHc) return;
  try {
    writeFileSync("/dev/tty", `\x1b]7501;state=${state}:app=opencode${extra}\x1b\\`);
  } catch {}
};

export const CorgiHcStatus = async () => {
  // A fresh session has rested before its first event.
  say("idle");
  return {
    event: async ({ event }) => {
      switch (event.type) {
        case "session.status":
          if (event.properties?.status?.type === "busy") say("working");
          else if (event.properties?.status?.type === "idle") say("done");
          break;
        case "session.idle":
          say("done");
          break;
        case "permission.asked":
          say("blocked", ":kind=permission");
          break;
        case "question.asked":
          say("blocked", ":kind=question");
          break;
        case "permission.replied":
          say("working");
          break;
        case "session.error":
          say("error");
          break;
      }
    },
  };
};
