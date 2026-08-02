'''
main spec
'''

from libspec import Spec
from . import app, observe


class MainSpec(Spec):
    def modules(self):
        return [app, observe]
