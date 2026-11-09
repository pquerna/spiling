# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
FROM python:3.12.12-slim-bookworm@sha256:593bd06efe90efa80dc4eee3948be7c0fde4134606dd40d8dd8dbcade98e669c AS runtime-base
RUN apt-get update && apt-get install -y --no-install-recommends libgomp1 libstdc++6 \
    && rm -rf /var/lib/apt/lists/*
FROM runtime-base AS wheel-builder
RUN apt-get update && apt-get install -y --no-install-recommends g++ \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY build-requirements.lock pymetis.lock pyamg.lock ./
RUN python -m pip install --no-cache-dir --only-binary=:all: --require-hashes -r build-requirements.lock \
    && python -m pip wheel --no-build-isolation --no-deps --require-hashes \
      --config-settings=compile-args=-j2 --wheel-dir=/wheels -r pymetis.lock -r pyamg.lock \
    && dpkg-query -W > /wheels/build-os-packages.txt \
    && python -m pip freeze > /wheels/build-python-packages.txt
FROM runtime-base
WORKDIR /opt/spiling-eval
COPY requirements.lock ./
COPY --from=wheel-builder /wheels /wheels
RUN python -m pip install --no-cache-dir --only-binary=:all: --require-hashes -r requirements.lock \
    && python -m pip install --no-index --no-deps /wheels/*.whl \
    && python -m pip freeze > /opt/spiling-eval/python-packages.txt \
    && dpkg-query -W > /opt/spiling-eval/os-packages.txt
COPY evaluate.py model.py geometry.py mechanics.py coupon.py reference.py quadratic.py quadratic_preconditioner.py quadratic_fields.py vault_mechanics.py ./
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 HOME=/tmp
LABEL org.spiling.purpose="offline-interior-evaluator"
USER 65532:65532
ENTRYPOINT ["python", "/opt/spiling-eval/evaluate.py"]
