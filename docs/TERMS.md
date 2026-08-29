# Terms of Use — ecofin

_Last updated: 29 August 2026_

## 1. What ecofin is

ecofin is a personal, non-commercial command-line tool. It reads the bank
accounts of the single individual who operates it, and stores the results on
that person's own computer for personal budgeting.

It is not a product, not a service, and is not offered to anyone else. There are
no users other than the operator.

## 2. Scope of use

ecofin is used exclusively to access accounts that the operator personally owns
and for which they have given explicit consent through their own bank's
authentication flow.

It is not used to access accounts belonging to any other person, and it is not
made available to any third party.

## 3. Account information services

Account data is obtained through the Enable Banking API, which acts as the
authorised account information service provider (AISP) under PSD2. Access to
bank accounts is governed by Enable Banking's own terms and by the terms of the
operator's agreement with their bank.

ecofin performs read-only operations. It cannot initiate payments, transfers, or
any other movement of funds, and contains no code capable of doing so.

## 4. No warranty

ecofin is provided as-is, without warranty of any kind. Balances and transaction
histories it displays are reproduced from data supplied by the bank and may be
incomplete, delayed, or inaccurate. Bank statements remain the authoritative
record.

## 5. Not financial advice

ecofin displays and summarises the operator's own financial data. Nothing it
produces constitutes financial, investment, tax, or legal advice.

## 6. No fees

ecofin is free. It is not sold, licensed, monetised, or advertised. No payment is
collected from anyone, and no revenue is derived from it in any form.

## 7. Responsibility for credentials

The operator is responsible for keeping their application private key and
configuration secure on their own machine. ecofin stores them with restrictive
filesystem permissions, but the security of the machine itself is the operator's
responsibility.

## 8. Consent and termination

PSD2 consent lasts at most 90 days and must then be renewed. The operator may
revoke it at any time from their bank's interface, at which point ecofin loses
all access to bank data. Locally stored data can be deleted at any time by
removing the application's data directory.

## 9. Governing law

These terms are governed by French law.

## 10. Contact

corinne.mario84120@gmail.com
