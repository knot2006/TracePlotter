# Security

TracePlotter runs with Administrator rights, sends probes to the network, updates itself and can
run a local server for AI agents. A flaw in any of these matters, so reports are welcome.

## Supported versions

Only the [latest release](https://github.com/knot2006/TracePlotter/releases/latest) gets fixes. The
app updates itself to it, so there are no patches for older versions.

## Reporting a vulnerability

Please do not open a public issue. Report it privately through
[**Security > Report a vulnerability**](https://github.com/knot2006/TracePlotter/security/advisories/new)
on GitHub. Say which version you used, what an attacker can do and how to reproduce it.

You should get an answer within a week. Once a fix is released, the advisory is published and
credits you, unless you would rather not be named.

## What is of most interest

- The updater: anything that makes the app install an exe that was not signed with the release key,
  or one older than the running version.
- The MCP server: a web page or another machine reaching it, or an agent doing more than its tools
  describe.
- Anything that lets a program without Administrator rights make the elevated app run code, write
  files outside its `TracePlotter-data` folder or change the firewall beyond the two rules in the
  README.
- Answers from the network (probe replies, reverse DNS, whois) that crash the app or run script in
  its window.

Probing itself is not a vulnerability: the app sends what the user, or an agent the user allowed,
asks it to send.
