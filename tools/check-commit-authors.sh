#!/usr/bin/env bash
# Checks that no commit names an AI tool, or a bot this repository doesn't
# use, as its author, its committer or a `Co-authored-by:` trailer. MapLibre's
# AI policy (https://github.com/maplibre/maplibre/blob/main/AI_POLICY.md)
# keeps the human contributor the author; credit a tool with an
# `Assisted-by:` trailer instead, which this leaves alone. A squash merge
# turns each commit's author into a co-author, so the authors of a pull
# request's commits count as much as its trailers.
#
#   check-commit-authors.sh <revision-range>   each commit in the range
#   check-commit-authors.sh --message <file>   a message being committed,
#                                              with the commit's identities
#                                              (hk's commit-msg hook)
#
# It exits 1 and lists what it rejects when it rejects anything.

set -euo pipefail

usage() {
    echo "usage: $0 <revision-range> | --message <commit-message-file>" >&2
    exit 2
}

# Commits as records of four fields: the commit, its author, its committer
# and its message. \036 ends a record and \037 ends a field.
if [[ $# -eq 2 && $1 == --message ]]; then
    message=$2
    # `git var` gives the identities this commit will have, `--author` and
    # the GIT_AUTHOR_* variables included; drop their timestamps.
    ident() { git var "$1" | sed -E 's/ [0-9]+ [-+][0-9]{4}$//'; }
    commits() {
        printf 'commit\037%s\037%s\037' \
            "$(ident GIT_AUTHOR_IDENT)" "$(ident GIT_COMMITTER_IDENT)"
        # What git strips from the message: comment lines, and everything
        # below `git commit --verbose`'s scissors line.
        sed -e '/^# -* >8 -*$/,$d' -e '/^#/d' "$message"
        printf '\036'
    }
elif [[ $# -eq 1 && $1 != -* ]]; then
    range=$1
    commits() {
        git log --format='%h%x1f%an <%ae>%x1f%cn <%ce>%x1f%B%x1e' "$range"
    }
else
    usage
fi

commits | awk '
BEGIN {
    RS = "\036"
    FS = "\037"
    # The bots that commit to this repository: Dependabot, and the release
    # workflow (github-actions, or the MapLibre release-plz app).
    allowed_bots["dependabot[bot]"] = 1
    allowed_bots["github-actions[bot]"] = 1
    allowed_bots["maplibre-release-plz[bot]"] = 1
    # Domains whose addresses only AI tools commit as.
    split("anthropic.com openai.com cursor.com ampcode.com warp.dev " \
          "opencode.ai factory.ai devin.ai cognition.ai aider.chat " \
          "windsurf.com codeium.com x.ai", domains, " ")
    for (i in domains) ai_domains[domains[i]] = 1
    # Whole names, once normalized, of AI tools.
    split("claude|copilot|codex|chatgpt|openai|anthropic|cursor|cursor agent|" \
          "amp|warp|opencode|gemini|devin|devin ai|aider|jules|junie|kiro|" \
          "windsurf|cline|grok", names, "|")
    for (i in names) ai_names[names[i]] = 1
    failed = 0
}

# Why an identity ("Name <email>") is rejected, or "" when it is not.
function rejection(identity,    raw, name, email, login, domain) {
    raw = identity
    email = ""
    if (match(identity, /<[^>]*>/)) {
        raw = substr(identity, 1, RSTART - 1)
        email = tolower(substr(identity, RSTART + 1, RLENGTH - 2))
    }
    raw = tolower(raw)
    gsub(/^[ \t]+|[ \t]+$/, "", raw)
    # Without parenthesized notes such as "(1m context)", and with
    # punctuation other than version dots and "[bot]" as spaces.
    name = raw
    gsub(/\([^)]*\)/, " ", name)
    gsub(/[^]a-z0-9.[]+/, " ", name)
    gsub(/^ +| +$/, "", name)

    # The GitHub account of a noreply address: [<id>+]<login>@users.noreply.github.com.
    login = ""
    if (email ~ /@users\.noreply\.github\.com$/) {
        login = email
        sub(/@.*/, "", login)
        sub(/^[0-9]+\+/, "", login)
    }
    domain = email
    sub(/^[^@]*@?/, "", domain)

    if (login ~ /(\[bot\]|-bot)$/ || name ~ /(\[bot\]|[- ]bot)$/) {
        if (!(login in allowed_bots) && !(raw in allowed_bots))
            return "a bot this repository does not use"
        return ""
    }
    if (domain in ai_domains) return "an AI tool\047s address"
    if (login == "copilot" || email == "copilot@github.com")
        return "GitHub Copilot\047s account"
    if (name in ai_names) return "an AI tool\047s name"
    # A tool and its model, or an account for an agent: "Claude Opus 5.5",
    # "GitHub Copilot Autofix", "GPT-5", "GLM 5.2", "someone-agent".
    if (name ~ /^(anthropic )?claude (opus|sonnet|haiku|fable|code|instant)( |$)/ ||
        name ~ / agent$/ ||
        name ~ /^(github )?copilot( |$)/ ||
        name ~ /^(openai |google |xai )?(gpt|chatgpt|codex|gemini|grok|glm|qwen|deepseek|llama|mistral|claude) ?[0-9]/)
        return "an AI tool\047s name"
    return ""
}

function check(commit, role, identity,    why) {
    gsub(/^[ \t]+|[ \t\r]+$/, "", identity)
    why = rejection(identity)
    if (why != "") {
        printf "%s: %s %s is %s\n", commit, role, identity, why
        failed = 1
    }
}

NF >= 4 {
    commit = $1
    sub(/^\n+/, "", commit)
    check(commit, "author", $2)
    check(commit, "committer", $3)
    lines = split($4, line, "\n")
    for (i = 1; i <= lines; i++) {
        if (tolower(line[i]) ~ /^[ \t]*co-authored-by[ \t]*:/) {
            identity = line[i]
            sub(/^[^:]*:/, "", identity)
            check(commit, "co-author", identity)
        }
    }
}

END {
    if (failed) {
        print "\nCredit an AI tool with an `Assisted-by:` trailer, not as an author, committer" \
              "\nor co-author (https://github.com/maplibre/maplibre/blob/main/AI_POLICY.md)."
        exit 1
    }
}
' >&2
