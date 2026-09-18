#!/usr/bin/env python3
"""Send one signed sample. This does not read Bluetooth data from a strap."""

import argparse
import hashlib
import hmac
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request


def bpm(value):
    number = int(value)
    if not 0 <= number <= 65535:
        raise argparse.ArgumentTypeError("must be an integer from 0 to 65535")
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://localhost:8080", help="server base URL")
    parser.add_argument("--heart-rate", type=bpm, required=True)
    parser.add_argument("--baseline-hr", type=bpm)
    args = parser.parse_args()
    secret = os.environ.get("SHARED_SECRET", "")
    if not secret.strip():
        parser.error("SHARED_SECRET must be set in the environment")
    parsed = urllib.parse.urlsplit(args.url)
    if parsed.scheme != "https" and not (
        parsed.scheme == "http" and parsed.hostname in ("localhost", "127.0.0.1", "::1")
    ):
        parser.error("use HTTPS, or HTTP on localhost for development")

    payload = {"heart_rate": args.heart_rate}
    if args.baseline_hr is not None:
        payload["baseline_hr"] = args.baseline_hr
    body = json.dumps(payload, separators=(",", ":")).encode()
    timestamp = str(int(time.time()))
    signature = hmac.new(secret.encode(), timestamp.encode() + b"." + body, hashlib.sha256).hexdigest()
    request = urllib.request.Request(
        args.url.rstrip("/") + "/api/v1/telemetry",
        data=body,
        headers={"Content-Type": "application/json", "X-Timestamp": timestamp, "X-Signature": signature},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            print(response.read().decode())
    except urllib.error.HTTPError as error:
        print(f"HTTP {error.code}: {error.read().decode()}", file=sys.stderr)
        return 1
    except urllib.error.URLError as error:
        print(f"Request failed: {error.reason}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
