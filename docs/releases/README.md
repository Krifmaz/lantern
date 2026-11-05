# Release notes

One file per release, `<MAJOR.MINOR.PATCH>.md`. The server builds them in for What's new, and the release
workflow uses the file as the release's notes. Adding `X.Y+1.0.md` (or `X+1.0.0.md`) in a pull request makes
the next release a MINOR (or MAJOR) one; without a file it is a PATCH release with the commit list as notes.
