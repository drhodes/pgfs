'''
main spec
'''

from libspec import Spec
from . import app, observe, offload, optimize, perf, replica


class MainSpec(Spec):
    def modules(self):
        return [app, observe, offload, optimize, perf, replica]

