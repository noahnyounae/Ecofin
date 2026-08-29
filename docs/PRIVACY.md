# Privacy Policy — ecofin

_Last updated: 29 August 2026_

## Summary

ecofin is a personal command-line tool that lets one individual read their own
bank accounts. It stores everything on that person's own computer. It has no
server, no user accounts, no analytics, and shares data with nobody.

## Who is responsible

ecofin is operated by a single private individual for their own personal use.
That person is both the sole operator and the only data subject.

**Contact:** ecofin@runiacorp.eu

## What data is processed

Via the Enable Banking API, acting under PSD2 account information services
(AIS), ecofin accesses the following, for accounts the operator personally owns
and has explicitly consented to share:

- account identifiers, IBAN, currency, and account name
- account balances
- transaction history: dates, amounts, descriptions, counterparty names, and
  bank transaction codes

No data belonging to any other person is accessed, requested, or stored.

## Why

Personal budgeting and expense tracking. The operator reads their own financial
history in order to understand and reduce their own spending. There is no other
purpose, no profiling, and no automated decision-making.

## Legal basis

Explicit consent, given by the operator directly to their own bank through the
bank's own strong customer authentication flow, as required by PSD2. Consent is
granted for a maximum of 90 days and must be renewed by repeating that flow. It
can be withdrawn at any time from the bank's own interface.

## Where data is stored

Locally, on the operator's personal computer only:

| Path | Contents |
|---|---|
| `~/.config/ecofin/ecofin.db` | Accounts, balances, transactions (SQLite) |
| `~/.config/ecofin/enablebanking.pem` | Application private key |
| `~/.config/ecofin/config.toml` | Application ID and configuration |

The private key and the configuration file are written with `0600` permissions
(readable by the operator's user account only), and the application refuses to
read them if their permissions are more permissive.

No data is uploaded anywhere. There is no cloud storage, no backend server, and
no hosted component of any kind.

## Who data is shared with

Nobody.

ecofin makes network requests to exactly one destination, `api.enablebanking.com`,
and only when the operator runs the `sync` or `link` commands. There is no
telemetry, no crash reporting, no analytics, and no third-party service of any
kind.

During the `link` command only, ecofin briefly runs a local HTTP listener on
`127.0.0.1` to receive the authorisation code returned by the bank. It is bound
to the loopback interface, accepts a single request, and shuts down immediately
afterwards. It is not reachable from outside the machine.

## How long data is kept

Locally stored transaction history is retained until the operator deletes it,
which they can do at any time by removing `~/.config/ecofin/ecofin.db`.

Access to the bank ends automatically when the PSD2 consent expires, at most 90
days after it was granted, regardless of what is stored locally.

## Rights

As the sole data subject, the operator has direct and complete control: all data
is on their own machine, in an open SQLite file they can read, export, or delete
at will. Consent can be withdrawn at the bank at any time.

## Security

- Secrets stored with restrictive filesystem permissions (`0600`), enforced on
  both write and read.
- All API traffic over HTTPS/TLS.
- Short-lived JWTs, signed locally with RS256; no long-lived access token is
  ever stored.
- The application private key never leaves the operator's machine.

## Changes

Any change to this policy will be published at this same URL with an updated
date.
