-- Domains of compose builds route to one service of the compose file.
ALTER TABLE domains ADD COLUMN compose_service TEXT;
