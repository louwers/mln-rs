#!/usr/bin/env bash
# Prints the AWS console's page of an AWS Device Farm run, given the run's ARN,
# arn:aws:devicefarm:<region>:<account>:run:<project ID>/<run ID>. Opening it
# needs a login to the account.
set -euo pipefail

IFS=: read -r _ _ _ region _ _ ids <<< "$1"
echo "https://$region.console.aws.amazon.com/devicefarm/home?region=$region#/mobile/projects/${ids%/*}/runs/${ids#*/}"
