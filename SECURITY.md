# Security policy

## Reporting a vulnerability

Do not open a public issue for a suspected credential leak, approval bypass,
host-key verification flaw, or remote code execution vulnerability.

Report it privately through
[GitHub Security Advisories](https://github.com/suiflex/SafeHell/security/advisories/new),
and include reproduction steps, affected versions, and impact.

## Supported versions

Only the latest release receives fixes. SafeHell is pre-1.0, so a breaking
change lands in a minor bump rather than a major one; upgrade before reporting
anything you have not reproduced against the current release.

## Outside the protection boundary

SafeHell keeps stored credentials away from the agent and puts a person in
front of every remote command. It does not defend against:

- direct shell access by the same operating-system user
- a compromised OS credential store
- a compromised SSH agent
- malicious remote output crafted to evade redaction
- commands the user explicitly approves, or that a project explicitly lists
  under `autoapprove.allow`

SafeHell also does not sandbox the remote shell. Output redaction and the agent
hooks are defense in depth, not a guarantee against every possible secret
representation or a bypass by another local process running as your user.
