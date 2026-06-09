CREATE TABLE IF NOT EXISTS internal_ips (
    ip TEXT PRIMARY KEY,
    department TEXT NOT NULL,
    owner TEXT NOT NULL,
    segment TEXT NOT NULL
);

INSERT INTO internal_ips (ip, department, owner, segment) VALUES
    ('10.0.0.99', 'engineering', 'dev_ops', 'workstation'),
    ('10.0.5.5', 'engineering', 'dev_ops', 'workstation'),
    ('10.0.2.0/24', 'security', 'sec_team', 'scanner_subnet'),
    ('192.168.1.0/24', 'infra', 'admin', 'dmz')
ON CONFLICT (ip) DO NOTHING;
