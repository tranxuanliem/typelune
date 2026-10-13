# Security policy

Typelune can observe keystrokes system-wide once you enable that feature. We take
reports seriously.

## Reporting

Please report vulnerabilities **privately** through GitHub's *Report a vulnerability*
button (Security → Advisories) on this repository. Do not open a public issue. We aim to
reply within 7 days.

## Scope and design

- **Off by default.** System-wide expansion never starts automatically. It needs the
  Accessibility permission, which you grant yourself; the app never changes permissions.
- **No storage of typed text.** Typed characters live only in a 64-character in-memory
  buffer used for matching. Nothing typed is logged, saved or sent anywhere. The app makes
  no network connections.
- **Injection is gated and fails closed.** Before replacing text, Typelune checks all of
  these:
  - the same focused app and text field as when the abbreviation was typed;
  - macOS Secure Keyboard Entry is not active;
  - no newer real input has arrived;
  - expansion is not paused or stopped.

  If the focused app can't be identified, nothing is injected.
- **Known limits.**
  - Secure Keyboard Entry covers only the apps that turn it on, so not every password field
    is detected.
  - Builds are ad-hoc signed and not notarized.

Supported versions: the latest release only (developer preview).
