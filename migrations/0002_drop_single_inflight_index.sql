-- Ordering is enforced by the advisory lock in the claim query. A partial unique
-- index cannot express "only for endpoints in ordered mode", and it also blocks
-- concurrent deliveries for endpoints that opted out of ordering.
drop index if exists deliveries_one_in_flight_per_endpoint;
