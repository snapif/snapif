# Agents

Every commit needs `git commit -s`. Sign-off email is `git config user.email`.

Crate and CLI identifier is `snapif`. Display name is Snapif.

Homepage and `.github/FUNDING.yml` stay empty until this file says
otherwise. GitHub topics and the README are the public page.

Do not depend on `bline-*`, `canact`, `wiremux`, or `wiremux-auth` in v1.

Local gate: `make check`.

A rule in this file changes in the same commit as the code that breaks
it. The commit message says which rule changed and why. Do not leave
the code and this file disagreeing. Do not keep a rule that blocks a
useful product change; update the rule in that commit.
