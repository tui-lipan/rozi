## Added
- The Extensions… Discover tab opens an installed extension in the Installed tab, where `Ctrl+K` removes it after a press-twice confirmation. ([#9001](https://github.com/tui-lipan/rozi/pull/9001))
- Café, Zażółć gęślą jaźń, 漢字, and 🎉 survive embedding byte for byte, together with a deliberately long line that keeps going well past the width of the What's new dialog so wrapping and any line-length limit in a tool that reads the binary are both exercised at once.
  A continuation line under the same bullet, indented by two spaces.

## Fixed
- A bullet without a pull request link, the way a direct commit to master is listed.

## Compatibility
- `rozi --release-notes` prints these bytes exactly, with no trailing newline added.
