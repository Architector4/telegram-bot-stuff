#!/usr/bin/env python3

# Really messy and hacky, but whatever lol

import re
import subprocess
import os

with open("src/database/mod.rs", "r") as f:
    data = f.read()


extract_queries = re.compile(r"sqlx::query\(\s*\"([\w\W]*?)\"", re.MULTILINE);

for match in extract_queries.finditer(data):
    query = match.group(1)
    if "CREATE TABLE" in query:
        continue

    if not query.endswith(';'):
        query += ";"

    query += "\n"

    print(query)

    subprocess.call(["sqlite3_expert", "-sql", query, "anti_nft_spam_bot.sqlite"])

