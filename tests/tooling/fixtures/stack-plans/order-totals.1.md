---
schema: zeroshot.plan/v1
id: order-totals.1
title: Show order totals at checkout
status: ready
owner: sam.k
base_commit: 3f2c1a9
depends_on: [order-totals]
---

# Show order totals at checkout

## Goal

The checkout page shows the order total from `GET /orders/{id}/total`.

## Context

- The checkout page is `web/checkout/Summary.tsx`.

## Acceptance criteria

- **AC-1** WHEN the checkout page loads THE SYSTEM SHALL show the total with its currency. Check:
  component test with a mocked endpoint.
- **AC-2** Existing checkout tests keep working. Check: `npm test -- checkout`.

## Constraints

- Do not change the order totals endpoint.

## Out of scope

- Showing totals in order history: a later plan.
