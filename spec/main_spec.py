'''
main spec
'''

from libspec import Spec
from . import app, observe, perf, replica


class MainSpec(Spec):
    def modules(self):
        return [app, observe, perf, replica]
