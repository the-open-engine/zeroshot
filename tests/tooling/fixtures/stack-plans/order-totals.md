---
schema: zeroshot.plan/v1
id: order-totals
title: Order totals endpoint
status: ready
owner: sam.k
base_commit: 3f2c1a9
---

# Order totals endpoint

## Goal

`GET /orders/{id}/total` returns the order total, including tax, in the order's currency.

## Context

- Orders are loaded in `api/orders.ts`; tax rules live in `pricing/tax.ts`.

## Acceptance criteria

- **AC-1** WHEN a client requests an existing order's total THE SYSTEM SHALL return the amount and
  currency. Check: integration test with a taxed order.
- **AC-2** Existing order endpoints keep working. Check: `npm test -- orders`.

## Constraints

- Do not change the stored order format.

## Decided approach

Compute the total from the stored line items with the existing tax rules.

## Decisions

- **D-1** Totals are computed on request rather than stored, because orders are small. (assumed)
