# Third-party licences

BPMN Simulator itself is licensed under the GNU Affero General Public License v3.0 or later
(see `LICENSE` in the project root).

## Rust crates

The binaries include code from the Rust crates listed in `Cargo.lock`. All of them are under
permissive licences (MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0, BSL-1.0,
Unlicense, 0BSD); where a crate offers a choice that includes a copyleft licence, the
permissive option is used. Each crate's licence text is in its source package on crates.io.

## Fonts

The desktop app embeds the default fonts of [egui](https://github.com/emilk/egui)
(crate `epaint_default_fonts`):

| Font | Licence | Text |
|---|---|---|
| Ubuntu Light | Ubuntu Font Licence 1.0 | `fonts/UFL.txt` |
| Noto Emoji | SIL Open Font License 1.1 | `fonts/OFL.txt` |
| Hack | MIT / Bitstream Vera Licence | `fonts/Hack-Regular.txt` |
| emoji-icon-font | MIT | `fonts/emoji-icon-font-mit-license.txt` |
| egui-icons (subset of emoji-icon-font) | MIT | `fonts/egui-icons.txt` |
