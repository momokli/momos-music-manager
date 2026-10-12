"""Shared state for the download service."""

import threading

# Limit concurrent CLI downloads (spotDL + deemix share this pool)
DOWNLOAD_SEM = threading.Semaphore(3)
