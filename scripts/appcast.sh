#!/usr/bin/env bash
# Writes the Sparkle update feed (appcast.xml) for a release, with the
# update and the feed itself signed:
#
#   scripts/appcast.sh VERSION ZIP ZIP_URL NOTES OUT
#
# ZIP is the release archive, published at ZIP_URL. NOTES is the release
# notes in Markdown. The private EdDSA key comes from SPARKLE_PRIVATE_KEY,
# and SIGN_UPDATE is the path to Sparkle's sign_update tool, from the same
# Sparkle version the app is built with.
set -euo pipefail

if [[ $# -ne 5 ]]; then
  echo "usage: $0 VERSION ZIP ZIP_URL NOTES OUT" >&2
  exit 2
fi
version="$1"
zip="$2"
zip_url="$3"
notes="$4"
out="$5"
: "${SPARKLE_PRIVATE_KEY:?the private EdDSA key}"
: "${SIGN_UPDATE:?the path to sign_update}"

sign() {
  printf '%s' "$SPARKLE_PRIVATE_KEY" | "$SIGN_UPDATE" --ed-key-file - "$@"
}

# sparkle:edSignature="..." length="..."
enclosure_signature="$(sign "$zip")"
# CDATA cannot hold "]]>"; split it across two sections.
release_notes="$(sed 's/]]>/]]]]><![CDATA[>/g' "$notes")"
published="$(LC_ALL=C date -u "+%a, %d %b %Y %H:%M:%S +0000")"

cat >"$out" <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>obojeno</title>
    <link>https://github.com/st1vc3/obojeno</link>
    <item>
      <title>obojeno $version</title>
      <pubDate>$published</pubDate>
      <sparkle:version>$version</sparkle:version>
      <sparkle:shortVersionString>$version</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>26.0</sparkle:minimumSystemVersion>
      <sparkle:fullReleaseNotesLink>https://github.com/st1vc3/obojeno/releases/tag/v$version</sparkle:fullReleaseNotesLink>
      <description sparkle:format="markdown"><![CDATA[$release_notes]]></description>
      <enclosure url="$zip_url" type="application/octet-stream" $enclosure_signature/>
    </item>
  </channel>
</rss>
XML

# Embeds the feed's own signature, which the app requires.
sign "$out"
