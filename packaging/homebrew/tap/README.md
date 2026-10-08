# wir-drei-digital/homebrew-tap

Homebrew formulae from wir-drei-digital.

```sh
brew install wir-drei-digital/tap/mailtriage
```

[mailtriage](https://github.com/wir-drei-digital/mailtriage) classifies your
email on your machine. After installing, run `mailtriage setup` once; it uses a
tested Himalaya on your `PATH` or installs a private one.
`brew upgrade mailtriage` installs new releases. On macOS the formula also
installs `mailtriage-tray`.

`Formula/mailtriage.rb` is written by mailtriage's release workflow from
`packaging/homebrew/mailtriage.rb.in` in that repository, and only for its
highest stable release. Change the template there, not the file here.

`.github/workflows/test.yml` installs and tests the formula on macOS and
Ubuntu after every push.
