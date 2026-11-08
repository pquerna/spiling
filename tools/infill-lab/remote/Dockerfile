# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
FROM python:3.12.12-slim-bookworm@sha256:593bd06efe90efa80dc4eee3948be7c0fde4134606dd40d8dd8dbcade98e669c
RUN apt-get update && apt-get install -y --no-install-recommends libgomp1 libstdc++6 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /opt/spiling-eval
COPY requirements.lock ./
RUN python -m pip install --no-cache-dir --only-binary=:all: --require-hashes -r requirements.lock \
    && python -m pip freeze > /opt/spiling-eval/python-packages.txt \
    && dpkg-query -W > /opt/spiling-eval/os-packages.txt
COPY evaluate.py ./
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 HOME=/tmp
LABEL org.spiling.purpose="offline-interior-evaluator"
USER 65532:65532
ENTRYPOINT ["python", "/opt/spiling-eval/evaluate.py"]
