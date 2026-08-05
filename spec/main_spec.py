'''
main spec
'''

from libspec import Spec
from . import app, observe, optimize, perf, replica


class MainSpec(Spec):
    def modules(self):
        return [app, observe, optimize, perf, replica]

