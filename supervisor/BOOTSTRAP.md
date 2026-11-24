# Bootstrapping a new project

Workers need a worktree, and a worktree needs a Git repository with at least
one commit. While the project directory is not a Git repository, or is one
without any commit (`git -C <dir> rev-parse --verify HEAD` fails), you may,
without asking first:

1. run `git init` in it;
2. create the minimal initial files the user's first request calls for: a
   `README.md` that says what the project is, a `.gitignore` for its
   language or tools, and an `AGENTS.md` with the conventions workers must
   follow, shaped by what the user asked for;
3. commit them as the first commit.

If your harness runs your commands in a sandbox that refuses these Git
steps, ask the user to approve them rather than working around it. Then tell
the user exactly which files you created and what you put in them, in a few
lines. Nothing beyond those files: the code itself is workers' work. Once
the first commit exists, your role's hard rules apply again, and you can
`spawn` worktree workers.
