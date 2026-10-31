---
id: no-ai-tracking-links
engine: phrase
scope: text
severity: error
# Provenance artifact, not prose style: ChatGPT appends utm_source=chatgpt.com
# (older builds: utm_source=openai) to every link it formats, so a citation
# hyperlink announces where it came from. lawlint-docx strips the same tags from
# .docx hyperlink targets (links.rs), which no text rule can see; keep the two
# value lists in step.
description: "Flags links tagged with an AI assistant's utm_source referral parameter."
rationale: "ChatGPT tags the links it formats with utm_source=chatgpt.com. The tag is invisible in rendered text but shows on hover, and tells the reader the link came from ChatGPT."
message: "This link carries a ChatGPT tracking tag."
examples:
  - bad: "See https://www.law.cornell.edu/uscode/text/15/78j?utm_source=chatgpt.com."
    good: "See https://www.law.cornell.edu/uscode/text/15/78j."
patterns:
  # Branch 1: the tag is the first of several query parameters, so keep the
  # `?` and drop the tag and its trailing `&`. Branch 2: anywhere else, drop
  # the tag and its leading separator, and put back the delimiter that ended
  # it (captured, because the regex has no lookahead). Requiring that
  # delimiter keeps `utm_source=chatgpt.com.example` from matching.
  - pattern: '(?i)(\?)utm_source=(?:chatgpt\.com|openai\.com|openai)&|[?&]utm_source=(?:chatgpt\.com|openai\.com|openai)([&#\s)\]>"''<]|$)'
    message: "This link carries a ChatGPT tracking tag (utm_source)."
    suggestion: "Remove the utm_source parameter from the URL."
    fixTemplate: "$1$2"
---
