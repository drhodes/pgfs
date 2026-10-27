'''
main spec
'''

import libspec.conventional_commits as conventional_commits
import libspec.diataxis as diataxis
from libspec import Spec

from . import app, observe, offload, optimize, perf, replica, shard


class MainSpec(Spec):

    def modules(self):
        return [
            app,
            observe,
            offload,
            optimize,
            perf,
            replica,
            shard,
            diataxis,
            conventional_commits,
        ]
