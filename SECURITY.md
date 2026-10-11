# Security

acctswap handles Claude Code login tokens, so a security bug here can expose someone's account.

**Report it privately:** use "Report a vulnerability" on the repository's Security tab (GitHub private vulnerability reporting). Please don't open a public issue, and never paste a token, a `.credentials.json` or a Keychain item into any report.

What counts, for example:
- a token reaching anything other than `api.anthropic.com`, or showing up in a process list, a log or a file readable by other users;
- a saved login written somewhere other than the places the README lists;
- a way for another program or user on the machine to read the saved logins.

Supported: the latest release. Expect a first answer within a week; this is a one-person project.
