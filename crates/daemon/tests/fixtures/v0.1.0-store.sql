-- The store a v0.1.0 daemon wrote (SQLite schema version 1), as sqlite3 .dump prints it,
-- of an install with two services, two deployments, env vars, a domain and a volume.
PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE servers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    host TEXT,
    user TEXT,
    port INTEGER,
    status TEXT NOT NULL DEFAULT 'unknown',
    proxy TEXT NOT NULL DEFAULT 'missing',
    docker_version TEXT,
    last_checked_at TEXT,
    created_at TEXT NOT NULL
);
INSERT INTO servers VALUES('t1gtw3osfl0s','local','local',NULL,NULL,NULL,'online','missing','29.9.3','2026-11-20 22:17:45.405113098+00:00','2026-11-20 22:17:45.376711980+00:00');
CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    created_at TEXT NOT NULL
);
INSERT INTO projects VALUES('jdovz7egifsw','shop',NULL,'2026-11-20 22:17:45.736119399+00:00');
CREATE TABLE environments (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (project_id, name)
);
INSERT INTO environments VALUES('xv8j5y5bmctt','jdovz7egifsw','production','2026-11-20 22:17:45.736120519+00:00');
INSERT INTO environments VALUES('e7sbflsvoe51','jdovz7egifsw','staging','2026-11-20 22:17:45.751931267+00:00');
CREATE TABLE services (
    id TEXT PRIMARY KEY,
    environment_id TEXT NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    server_id TEXT NOT NULL REFERENCES servers(id),
    name TEXT NOT NULL,
    source TEXT NOT NULL,
    port INTEGER,
    command TEXT,
    restart TEXT NOT NULL,
    health_path TEXT,
    stopped INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    UNIQUE (environment_id, name)
);
INSERT INTO services VALUES('paanq51yi1te','xv8j5y5bmctt','t1gtw3osfl0s','web','{"type":"image","image":"nginx:alpine"}',80,NULL,'unless-stopped','/',0,'2026-11-20 22:17:45.767689695+00:00');
INSERT INTO services VALUES('n2ksekpphelq','e7sbflsvoe51','t1gtw3osfl0s','api','{"type":"image","image":"traefik/whoami"}',80,NULL,'unless-stopped',NULL,0,'2026-11-20 22:17:50.362689500+00:00');
CREATE TABLE env_vars (
    service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    secret INTEGER NOT NULL,
    PRIMARY KEY (service_id, key)
);
INSERT INTO env_vars VALUES('paanq51yi1te',0,'GREETING','hello',0);
INSERT INTO env_vars VALUES('paanq51yi1te',1,'EXTRA','1',0);
CREATE TABLE domains (
    id TEXT PRIMARY KEY,
    service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    host TEXT NOT NULL UNIQUE,
    port INTEGER,
    tls INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
INSERT INTO domains VALUES('cot862dt9ajv','paanq51yi1te','web.example.test',NULL,1,'2026-11-20 22:17:45.767713575+00:00');
CREATE TABLE volumes (
    id TEXT PRIMARY KEY,
    service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    mount_path TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (service_id, name)
);
INSERT INTO volumes VALUES('g8yscgvtdytm','paanq51yi1te','data','/data','2026-11-20 22:17:45.767716615+00:00');
CREATE TABLE deployments (
    id TEXT PRIMARY KEY,
    service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    server_id TEXT NOT NULL,
    number INTEGER NOT NULL,
    status TEXT NOT NULL,
    trigger TEXT NOT NULL,
    commit_sha TEXT,
    commit_message TEXT,
    image TEXT,
    container_id TEXT,
    steps TEXT NOT NULL,
    error TEXT,
    created_at TEXT NOT NULL,
    finished_at TEXT,
    UNIQUE (service_id, number)
);
INSERT INTO deployments VALUES('kb583z1vdubl','paanq51yi1te','t1gtw3osfl0s',1,'superseded','api',NULL,NULL,'nginx:alpine','05748f8674e978dcab6bcbf3e515a3d6ac05d25809a3571591434c967b55176f','[{"kind":"pull","status":"done","detail":"nginx:alpine","started_at":"2026-11-20T22:17:45.775450734Z","finished_at":"2026-11-20T22:17:46.052520609Z"},{"kind":"start","status":"done","detail":"web-xv8j5y-1","started_at":"2026-11-20T22:17:46.054094974Z","finished_at":"2026-11-20T22:17:46.236235774Z"},{"kind":"health","status":"done","detail":"GET / -> 200","started_at":"2026-11-20T22:17:46.237094709Z","finished_at":"2026-11-20T22:17:48.243027584Z"},{"kind":"switch","status":"done","detail":"#1 live","started_at":"2026-11-20T22:17:48.244636938Z","finished_at":"2026-11-20T22:17:50.290005867Z"},{"kind":"cleanup","status":"done","detail":"removed 0 old containers","started_at":"2026-11-20T22:17:50.291944879Z","finished_at":"2026-11-20T22:17:50.312982456Z"}]',NULL,'2026-11-20 22:17:45.770477764+00:00','2026-11-20 22:17:50.290005867+00:00');
INSERT INTO deployments VALUES('jgntbbd7jbhc','paanq51yi1te','t1gtw3osfl0s',2,'live','cli',NULL,NULL,'nginx:alpine','8b80f279d4119776a40591e8e2b180ec21939d8ea153e8e2caca5fe237d0279d','[{"kind":"pull","status":"done","detail":"nginx:alpine","started_at":"2026-11-20T22:17:52.470254153Z","finished_at":"2026-11-20T22:17:52.704512176Z"},{"kind":"start","status":"done","detail":"web-xv8j5y-2","started_at":"2026-11-20T22:17:52.706287930Z","finished_at":"2026-11-20T22:17:52.910225465Z"},{"kind":"health","status":"done","detail":"GET / -> 200","started_at":"2026-11-20T22:17:52.911411510Z","finished_at":"2026-11-20T22:17:54.916496776Z"},{"kind":"switch","status":"done","detail":"#2 live","started_at":"2026-11-20T22:17:54.918133479Z","finished_at":"2026-11-20T22:17:56.965193742Z"},{"kind":"cleanup","status":"done","detail":"removed 1 old container","started_at":"2026-11-20T22:17:56.967000395Z","finished_at":"2026-11-20T22:17:57.382065054Z"}]',NULL,'2026-11-20 22:17:52.466187329+00:00','2026-11-20 22:17:56.965193742+00:00');
CREATE TABLE activity (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at TEXT NOT NULL,
    level TEXT NOT NULL,
    service_id TEXT,
    service_name TEXT,
    project_id TEXT,
    message TEXT NOT NULL,
    detail TEXT
);
INSERT INTO activity VALUES(1,'2026-11-20 22:17:45.737417014+00:00','info',NULL,NULL,'jdovz7egifsw','Project shop created',NULL);
INSERT INTO activity VALUES(2,'2026-11-20 22:17:45.752905143+00:00','info',NULL,NULL,'jdovz7egifsw','Environment staging created in shop',NULL);
INSERT INTO activity VALUES(3,'2026-11-20 22:17:45.769321489+00:00','info','paanq51yi1te','web','jdovz7egifsw','Service created','nginx:alpine');
INSERT INTO activity VALUES(4,'2026-11-20 22:17:45.773750931+00:00','info','paanq51yi1te','web','jdovz7egifsw','Deploy #1 started','api');
INSERT INTO activity VALUES(5,'2026-11-20 22:17:50.314400570+00:00','success','paanq51yi1te','web','jdovz7egifsw','Deploy #1 is live','4s');
INSERT INTO activity VALUES(6,'2026-11-20 22:17:50.363814855+00:00','info','n2ksekpphelq','api','jdovz7egifsw','Service created','traefik/whoami');
INSERT INTO activity VALUES(7,'2026-11-20 22:17:52.468884198+00:00','info','paanq51yi1te','web','jdovz7egifsw','Deploy #2 started','cli');
INSERT INTO activity VALUES(8,'2026-11-20 22:17:57.384066716+00:00','success','paanq51yi1te','web','jdovz7egifsw','Deploy #2 is live','4s');
INSERT INTO activity VALUES(9,'2026-11-20 22:17:57.512923508+00:00','info','paanq51yi1te','web','jdovz7egifsw','Environment variables updated','2 variables, applied on next deploy');
DELETE FROM sqlite_sequence;
INSERT INTO sqlite_sequence VALUES('activity',9);
CREATE INDEX deployments_status ON deployments(status);
COMMIT;
PRAGMA user_version = 1;
