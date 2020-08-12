# lokalise-bulk-add-keys

A small CLI to add a bunch of translations to Lokalise at once.

## Usage

Install (or update) with `cargo install --git https://github.com/undo-insurance/lokalise-bulk-add-keys.git`.

Set an environment variable called `LOKALISE_API_TOKEN` with a read+write API token. You can make one [here](https://app.lokalise.com/profile#apitokens).

Write a YAML file containing the keys you want to add:

```yaml
keys:
    - key: greeting
      translation: Hello [%s:name]!
      tags: # defaults to no tags
          - tag_one
          - tag_two

    - key: singlular_and_plural
      translations: # the plural 's'
          singular: Singular text
          plural: Plural text

    - key: multi_line
      translation: |- # this means multi line string without trailing newline
          Line one
          and line two
```

Then run

```
$ lokalise-bulk-add-keys --project Undo the_file.yaml
```

The strings will be added to the default locale of the project.
