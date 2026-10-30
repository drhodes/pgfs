'''
main spec
'''

import libspec.conventional_commits as conventional_commits
import libspec.diataxis as diataxis
from libspec import Spec

from . import (
    app,
    branch,
    changefeed,
    concurrency,
    consistency,
    durability,
    failure,
    integrity,
    observe,
    offload,
    ops,
    optimize,
    perf,
    posix,
    replica,
    security,
    shard,
    space,
    storage,
    verify,
)


class MainSpec(Spec):

    def modules(self):
        return [
            app,
            storage,
            durability,
            consistency,
            posix,
            failure,
            concurrency,
            branch,
            changefeed,
            space,
            integrity,
            security,
            verify,
            ops,
            observe,
            offload,
            optimize,
            perf,
            replica,
            shard,
            diataxis,
            conventional_commits,
        ]
